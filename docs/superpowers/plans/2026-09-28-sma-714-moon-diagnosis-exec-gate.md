<!-- moon-diagnosis:ok -->
# SMA-714: `repo:moon-diagnosis-exec` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the gate `repo:moon-diagnosis-exec`, which runs the root `CLAUDE.md` moon-diagnosis procedure (Step 1, Step 2 and Step 2a) against a real failed `moon ci` report, so that a broken Step 1 query reds CI.

**Architecture:** One bash script, `ci/moon-diagnosis/run.sh`, with three modes (`--self-test`, `--negative-control`, real run). The real run extracts the Step 1 `jq` query from the `CLAUDE.md` block, checks its shape, makes a throwaway git workspace with five tasks, runs a nested `moon ci` in it under an `env -i` allowlist environment, runs the documented query with `jq` directly, and compares the output and the state files with fixed expectations. The gate gets the seven registrations of a `repo:*` gate, and the docs that pointed to a missing gate are narrowed.

**Tech Stack:** bash (3.2-compatible), POSIX `awk`, `jq`, `python3` (Step 2a time conversion only), `git`, moon 2.5.3 through proto; Python 3 for the `ci_targets.py` registry.

**Spec:** `docs/superpowers/specs/2026-09-28-sma-714-moon-diagnosis-exec-gate-design.md` (APPROVED, with the approval decisions D1-D6). Executors read the spec and this plan.

## Global Constraints

- Gate name: `repo:moon-diagnosis-exec` (D1). The moon task key is `moon-diagnosis-exec`.
- Steps 1, 2 and 2a are in scope, including the Step 2a doc-text change in the root `CLAUDE.md` block (D2).
- Step 2a tolerance: `0 <= finishedAt_ms - lastRunTime <= 1000` ms (D3). Compare in integer microseconds.
- The root `CLAUDE.md` has two gated marker blocks (`ci-targets`, `moon-diagnosis`). Each marker occurs exactly once in the file. Do not quote a marker anywhere else in the file, also not in backticks (D4).
- The new target goes after `:helm-render` and before `:test-e2e`, in `ci.yml`'s `T=(…)` array AND in the `ci-targets` block, byte for byte the same order (D4, spec §6).
- The local-review stage runs at most ONE CodeRabbit CLI pass. If it is rate limited, skip it and say so in the PR (D5).
- The Mac can be in the 512-byte small-pipe state (SMA-612). Run bash-5 gates in a Linux container (`docker run ubuntu:24.04`) when the host is in that state (D6).
- `run.sh` runs under `/bin/bash` 3.2.57 and bash 5: no `mapfile`, no `declare -A`, no here-string, no heredoc, no pipe into an early-exit reader (`grep -q`, `grep -m`, `head`, `awk … exit`, `sed … q`) (spec §2, `ci/CLAUDE.md` lines 292-316).
- `run.sh` line 1-5 carry the SPDX header, and a `# moon-diagnosis:ok` comment. `run.sh` exports `PROTO_REPORTER=text` at the top (SMA-609).
- Every fixture `git` call runs with `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1`, and the fixture sets local `commit.gpgsign false`, `tag.gpgsign false`, `maintenance.auto false`, `gc.auto 0` directly after `git init` (M10, SMA-708).
- The nested moon runs with the allowlist `env -i HOME PATH TMPDIR LANG PROTO_HOME PROTO_REPORTER=text MOON_WORKSPACE_ROOT=<fixture>` (M3, M4, M5, M9).
- Exit codes: 0 clean, 1 an assertion failed, 2 an infrastructure error. Every rc 2 message holds `infrastructure error (rc=2)`.
- Only these files contain the token `ciReport` after the change: `ci/moon-diagnosis/run.sh` and `ci/moon-diagnosis/README.md` (both carry `moon-diagnosis:ok`), the root `CLAUDE.md` block (allowlisted), and the files that already hold it on origin/main. No pinned line, no `moon.yml` comment and no `ci_targets.py` row holds the token.
- New repo text is written in ASD-STE100 Simplified Technical English.
- Conventional commits with the `ci` scope. Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never `--no-verify`, `--no-gpg-sign`, `--amend`, `git reset`. Before each commit, `git -C <worktree> branch --show-current` must print `feature/sma-714-moon-diagnosis-exec-gate`.
- Mutation runs are authorized (Sven, 2026-09-28): a temporary edit, a run that must go red, then a restore with the Edit tool (never `git checkout --`). Confirm the restore with `git diff`. Never commit a mutation.
- Do not install host software. Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.

## Measurements taken for this plan (2026-09-28)

The spec left four values to the plan. They were measured with the exact fixture of this plan
(the script in Task 1), on macOS (moon 2.5.3, proto shims, Homebrew jq 1.8.2, Python 3.14.7) and in
`docker run ubuntu:24.04` (aarch64, bash 5.2.21, jq 1.7, Python 3.12.3, proto 0.61.1 and moon
2.5.3 installed in the container). T7 is therefore done before this plan is final (D3).

- **P1. Action status of `slow`.** The ACTION status is `failed` on macOS and on Linux (6 of 6
  Linux runs). Only its `task-execution` OPERATION has the status `timed-out`. So the Step 1 query
  (`select(.status=="failed")`) returns the `slow` row, and the gate-owned fixture-run check
  expects `RunTask(probe:slow)=failed`.
- **P2. Negative-control row kind.** The pre-SMA-711 query makes `exec` a JSON OBJECT
  (`{"command":null,"exitCode":null}`) for each of the four failed tasks. The exact expected row
  set is `bad-exec RunTask(probe:fail3) object`, `bad-exec RunTask(probe:missing) object`,
  `bad-exec RunTask(probe:multi) object`, `bad-exec RunTask(probe:slow) object`.
- **P3. Linux data (T7).** Step 1 output, `command not found` text
  (`bash: line 1: no-such-binary-xyz: command not found`), exit codes (3, 4, 127, `null` for the
  timed-out task; `lastRun.json` has `-1` for it), and the `multi/stdout.log` line `a` are
  identical on macOS and Linux. Step 2a difference `finishedAt_ms - lastRunTime` on Linux over
  24 samples: minimum 0.067 ms, maximum 1.031 ms, no negative value. The macOS sample of this
  session: about 1.05 ms. Spec M7 (macOS, 240 samples): 0.127 ms to 15.611 ms. The 1000 ms bound
  holds on both.
- **P4. Run time.** One nested `moon ci` takes 6 s to 8 s of wall time on both hosts (spec M1 said
  "a few seconds"; the `slow` task is 2 s of it). Measured modes: `--self-test` 0.14 s (Linux) and
  0.47 s (macOS); real run 7.4 s (Linux) and 6.9 s (macOS); `--negative-control` 7.0 s (Linux)
  and 7.1 s (macOS). The moon task therefore costs about 15 s.
- **P5. M-f arm.** A fixture pin of `2.5.2` (not installed) makes `moon --version` fail with
  `proto::commands::run::missing_tool` and exit 1. The gate reports rc 2 from its `moon --version`
  check (the "failed start" arm).
- **P6. The spec's M5 did not reproduce.** With `CI=true`, `GITHUB_ACTIONS=true`, an unknown
  `GITHUB_SHA`, `GITHUB_BASE_REF`, `GITHUB_HEAD_REF`, `GITHUB_EVENT_NAME=pull_request` and
  `GITHUB_REF` set, a nested `moon ci … --base <sha>` printed the real base sha on macOS, also with
  a plain inherited environment. The allowlist stays (it is also required by M3 and M4). The gate
  adds one positive check (not in the spec): the nested stdout must contain
  `Base revision: <base sha>`, else rc 2. This makes T6 a permanent assertion. A mutation that
  passes `--base HEAD~1` proves that the check bites (Task 4, M-c2).
- **P7. The draft script passed all three modes** on macOS under `/bin/bash` 3.2.57 and under
  Homebrew bash 5.3.15 (self-test only), and in the Linux container. It also passed T6 (the
  outer environment had the `CI`/`GITHUB_*` variables, `PROTO_MOON_VERSION=2.4.6` and
  `MOON_WORKSPACE_ROOT=/nonexistent`). M-a gave rc 1 with three `wrong-exec` rows (fail3,
  missing, multi; `slow` has a `null` exit code in both forms). M-b gave rc 1 with four
  `wrong-exec` rows. The AC6 set of Task 4 Step 1 and M-h gave the rows and rc that Task 4
  predicts. The check-13 scan (`early_exit_reader_rows`) found no row in the script.
  The `ci_targets.py` edit script (Task 2) applied cleanly to origin/main `52d51f82`, `--self-test`
  printed `ci-targets self-test OK`, and ruff reported `All checks passed!`.

## Review Focus

These five inputs or conditions are implied by the spec, but no unit test in the gate exercises
them. Each one has a test step in the task named.

1. **An outer `moon ci` that runs this task.** In CI the gate runs inside `moon ci`. If moon
   rewrote the outer workspace's report while the task runs, the outer-report hash compare would
   give a false rc 2. Expected: rc 0. Test: Task 2 Step 9 (`moon ci :moon-diagnosis-exec` in the
   worktree).
2. **The host signs commits and 1Password is locked.** A person runs the gate on the development
   Mac with a locked vault. Expected: rc 0, the fixture commits do not sign. Test: Task 4 Step 6
   (a HOME with a global config that forces signing through `/usr/bin/false`).
3. **A repository path that contains a space.** Expected: rc 0 in all three modes. Test: Task 4
   Step 7.
4. **A different `jq` version.** The Homebrew `jq` on the Mac exited 5 on a non-JSON input; other
   versions can exit 2. Expected: the
   self-test and the verdicts do not depend on the value. Test: the self-test normalizes the rc
   (`rc=N`), and Task 4 Step 5 runs all modes with jq 1.7 in the Linux container.
5. **Homebrew bash 5 on the small-pipe host.** Expected: no hang, because the script uses no
   here-string and no heredoc. Test: Task 1 Step 4 runs `--self-test` under
   `/opt/homebrew/bin/bash` with a 120 s alarm.

---

## File structure

| File | Change | Responsibility |
|---|---|---|
| `ci/moon-diagnosis/run.sh` | Create | The gate: seven units, three modes. |
| `ci/moon-diagnosis/README.md` | Create | What the gate asserts, exit codes, limits. |
| `moon.yml` | Modify | New task `moon-diagnosis-exec` at the end; new `ci/moon-diagnosis/**/*` input of `affected-smoke`. |
| `.github/workflows/ci.yml` | Modify | `:moon-diagnosis-exec` in `T=(…)` (line 267). |
| `CLAUDE.md` (root) | Modify | `ci-targets` block (line 146); the `moon-diagnosis` block sentence (lines 188-190) and Step 2a text (lines 202-204). |
| `ci/affected-graph/ci_targets.py` | Modify | `REQUIRED_REPO_TASKS`, `SELF_TASK_EXPECTED_GLOBS`, `SELF_SCHEDULED_GATES`, `MOON_DIAGNOSIS_SH_CALL_SITES`, the tenth haystack of `check_self_invocation`, its self-test battery. |
| `ci/actionlint/run.sh` | Modify | `'ci/moon-diagnosis/**/*'` in `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (after line 2163). |
| `ci/actionlint/README.md` | Modify | Narrow L29 (lines 548-551) and L32 (lines 561-569). |
| `ci/CLAUDE.md` | Modify | New section at the end: a nested `moon` call that targets a fixture workspace. |

## Unit and row reference (for all tasks)

`run.sh` functions and what they print:

| Function | Arguments | Output |
|---|---|---|
| `extract_step1_query` | `CLAUDE_MD OUT` | one word: `ok`, `no-block`, `no-step1`, `no-fence`, `unclosed-fence`, `multiple-step1`; on `ok`, `OUT` holds the dedented fence body |
| `query_shape_verdict` | `QUERY_FILE PROG_OUT` | `ok` or `bad-shape <reason>`; on `ok`, `PROG_OUT` holds the program and `PROG_OUT.flags` one flag per line |
| `make_fixture` | `DIR PIN` | prints the sha of commit 1; returns 2 on a failed `git` call |
| `run_nested_moon` | `DIR BASE PIN` | nothing; exits 2 (`die_infra`) on every infrastructure fault |
| `run_step1_query` | `PROG_FILE FLAGS_FILE DIR OUT` | nothing, or `query-error rc=<n>` |
| `step1_verdict` | `OUT` | rows `empty-output`, `missing-row <label>`, `bad-exec <label> <object/null/absent/…>`, `wrong-exec <label>`, `unexpected-row <label>`, `verdict-error step1 rc=<n>` (sorted) |
| `step2_verdict` | `DIR` | rows `step2-missing-file <task>/<file>`, `step2-missing-no-command-not-found`, `step2-multi-no-line-a`, `step2a-no-action <task>`, `step2a-no-lastrun <task>`, `step2a-out-of-range <task> <ms>`, `verdict-error step2 rc=<n>` |
| `swap_step1_body` | `MD BODY_FILE OUT` | `ok` or `no-fence` |

The real path prints each row as `ROW <row>` on stdout. The negative control reads the `ROW `
lines of its child. Labels have the form `RunTask(probe:<task>)`.

---

### Task 1: The gate script and its README

**Files:**
- Create: `ci/moon-diagnosis/run.sh`
- Create: `ci/moon-diagnosis/README.md`

**Interfaces:**
- Consumes: nothing from other tasks. Reads the repo `.prototools` (`^moon = "X.Y.Z"$`) and the root `CLAUDE.md`.
- Produces: `bash ci/moon-diagnosis/run.sh [--self-test | --negative-control]`, exit 0/1/2. Task 2 pins 59 of its lines verbatim (the tuple in Task 2 Step 3), so do NOT reformat any line of the script.

- [ ] **Step 1: Check the worktree and the base**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && git branch --show-current && git status --short && ls ci/moon-diagnosis 2>&1
```
Expected: `feature/sma-714-moon-diagnosis-exec-gate`, a clean tree, and `No such file or directory`.

- [ ] **Step 2: Write `ci/moon-diagnosis/run.sh` with exactly this content**

The self-test tables ARE the tests of the pure units (units 1, 2, 6, 7 and the swap helper). They
assert the exact verdict text of 37 cases and then the case count.

````bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# moon-diagnosis:ok — this file names the moon ci report file on purpose (check 12 of
# repo:actionlint). It runs the CORRECTED procedure, and it holds one broken query that is the
# negative-control input (see OLD_STEP1_LINES below).
#
# repo:moon-diagnosis-exec — runs the root CLAUDE.md moon-diagnosis procedure (Step 1, Step 2 and
# Step 2a) against a real failed `moon ci` report in a throwaway fixture workspace (SMA-714).
#
#   run.sh                     the real run on the working-tree CLAUDE.md
#   run.sh --self-test         fixture tables for the pure units; no moon and no git
#   run.sh --negative-control  the real path on a CLAUDE.md copy that holds the pre-SMA-711 query;
#                              it must fail with exactly the expected row set
#
# Exit codes: 0 pass | 1 an assertion failed | 2 infrastructure error. Every rc 2 message holds
# the text `infrastructure error (rc=2)`.
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no heredoc,
# no pipe into an early-exit reader (ci/actionlint check 13), and a "+" guard on every
# possibly-empty array. Everything it writes lands under one mktemp -d directory, removed on EXIT.
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment. This script does not capture proto
# output today; the export covers a later capture (standing rule, SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/moon-diagnosis"
REPORT_REL=".moon/cache/ciReport.json"
SELF_TEST_CASES=37
FAILED_TASKS="fail3 multi missing slow"

die_infra() { printf 'moon-diagnosis-exec: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }

SELFTEST=0
NEGATIVE=0
CLAUDE_MD="$REPO_ROOT/CLAUDE.md"
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        SELFTEST=1; shift ;;
    --negative-control) NEGATIVE=1; shift ;;
    --claude-md)        [ $# -ge 2 ] || die_infra "--claude-md needs a path"; CLAUDE_MD="$2"; shift 2 ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

TMP="$(mktemp -d)" || die_infra "mktemp -d failed"
trap 'rm -rf "$TMP"' EXIT

# moon-diagnosis:ok — NEGATIVE-CONTROL INPUT, BROKEN ON PURPOSE. This is the pre-SMA-711 Step 1
# query (git show 81451a82^:CLAUDE.md). It reads command and exitCode from the operation, not
# from .meta, so it prints null. Never copy it into documentation.
OLD_STEP1_LINES=(
  "jq '.actions[] | select(.status==\"failed\")"
  "    | {label, error,"
  "       exec: (.operations[] | select(.meta.type==\"task-execution\") | {command, exitCode})}' \\"
  "   $REPORT_REL"
)

# The exact row set that the old query must produce (step1_verdict rows, sorted).
NEGATIVE_WANT_ROWS=(
  "bad-exec RunTask(probe:fail3) object"
  "bad-exec RunTask(probe:missing) object"
  "bad-exec RunTask(probe:multi) object"
  "bad-exec RunTask(probe:slow) object"
)

# ---------------------------------------------------------------------------------------------
# Unit 1. Prints one verdict word: ok | no-block | no-step1 | no-fence | unclosed-fence |
# multiple-step1. On ok, OUT holds the fence body with the common leading indent removed.
extract_step1_query() {
  local md="$1" out="$2"
  [ -f "$md" ] && [ -r "$md" ] || { echo "no-block"; return 0; }
  awk -v out="$out" '
    function lead(s) { match(s, /^[ \t]*/); return RLENGTH }
    index($0, "<!-- moon-diagnosis:begin -->") { nb++; if (nb == 1) { inblk = 1; next } }
    index($0, "<!-- moon-diagnosis:end -->")   { ne++; if (inblk) { inblk = 0; done = 1 } next }
    !inblk { next }
    {
      if (state == 0) { if (index($0, "Step 1")) state = 1; next }
      if (state == 1) {
        if ($0 ~ /^[ \t]*```bash[ \t]*$/) { state = 2; n = 0; next }
        if (index($0, "**Step ")) { state = 9 }
        next
      }
      if (state == 2) {
        if ($0 ~ /^[ \t]*```[ \t]*$/) { state = 3; next }
        body[++n] = $0; next
      }
      if (state == 3) {
        if (index($0, "**Step 1")) { multi = 1 }
        if (index($0, "**Step 2")) { state = 4 }
        else if ($0 ~ /^[ \t]*```bash[ \t]*$/) { multi = 1 }
        next
      }
      if (state == 4) { if (index($0, "**Step 1")) multi = 1; next }
    }
    END {
      if (nb != 1 || ne != 1 || !done) { print "no-block"; exit 0 }
      if (state == 0) { print "no-step1"; exit 0 }
      if (state == 1 || state == 9) { print "no-fence"; exit 0 }
      if (state == 2) { print "unclosed-fence"; exit 0 }
      if (multi) { print "multiple-step1"; exit 0 }
      min = -1
      for (i = 1; i <= n; i++) {
        if (body[i] ~ /^[ \t]*$/) continue
        l = lead(body[i]); if (min < 0 || l < min) min = l
      }
      if (min < 0) min = 0
      printf "" > out
      for (i = 1; i <= n; i++) print substr(body[i], min + 1) >> out
      close(out)
      print "ok"
    }
  ' "$md"
}

# Unit 2. Prints: ok | bad-shape <reason>. On ok, PROG_OUT holds the jq program and
# PROG_OUT.flags holds one flag per line. Pure bash: no subprocess reads the doc text.
query_shape_verdict() {
  local qf="$1" pout="$2" text rest tok prog after nl
  nl='
'
  [ -f "$qf" ] || { echo "bad-shape no-query-file"; return 0; }
  text="$(cat "$qf")" || { echo "verdict-error shape rc=$?"; return 0; }
  text="${text//\\$nl/ }"
  rest="${text#"${text%%[![:space:]]*}"}"
  case "$rest" in
    "jq "*|"jq	"*) rest="${rest#jq}" ;;
    *) echo "bad-shape not-jq"; return 0 ;;
  esac
  : > "$pout.flags"
  while :; do
    rest="${rest#"${rest%%[![:space:]]*}"}"
    case "$rest" in
      -*) tok="${rest%%[[:space:]]*}"
          case "$tok" in
            -r|-c|-s|-e) printf '%s\n' "$tok" >> "$pout.flags"; rest="${rest#"$tok"}" ;;
            *) echo "bad-shape flag-not-allowed $tok"; return 0 ;;
          esac ;;
      *) break ;;
    esac
  done
  case "$rest" in
    "'"*) rest="${rest#\'}" ;;
    *) echo "bad-shape no-single-quoted-program"; return 0 ;;
  esac
  case "$rest" in
    *"'"*) ;;
    *) echo "bad-shape unclosed-program"; return 0 ;;
  esac
  prog="${rest%%\'*}"
  after="${rest#*\'}"
  after="${after#"${after%%[![:space:]]*}"}"
  after="${after%"${after##*[![:space:]]}"}"
  [ "$after" = "$REPORT_REL" ] || { echo "bad-shape tail-is-not-the-report-path"; return 0; }
  [ -n "$prog" ] || { echo "bad-shape empty-program"; return 0; }
  printf '%s' "$prog" > "$pout"
  echo "ok"
}

# Unit 3. Makes the fixture workspace in DIR and prints the sha of commit 1.
make_fixture() {
  local dir="$1" pin="$2"
  mkdir -p "$dir/probe" "$dir/.moon" || return 2
  printf 'moon = "%s"\n' "$pin" > "$dir/.prototools"
  printf '%s\n' \
    "projects:" \
    "  probe: 'probe'" \
    "vcs:" \
    "  client: 'git'" \
    "  defaultBranch: 'main'" > "$dir/.moon/workspace.yml"
  printf '.moon/cache\n' > "$dir/.gitignore"
  printf 'one\n' > "$dir/probe/file.txt"
  printf '%s\n' \
    "tasks:" \
    "  fail3:" \
    "    script: 'exit 3'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" \
    "  multi:" \
    "    script: |" \
    "      echo a" \
    "      exit 4" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" \
    "  missing:" \
    "    command: 'no-such-binary-xyz'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" \
    "  slow:" \
    "    command: 'sleep 20'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false, timeout: 2 }" \
    "  ok:" \
    "    command: 'true'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" > "$dir/probe/moon.yml"
  fx_git -C "$dir" init -q -b main || return 2
  fx_git -C "$dir" config commit.gpgsign false || return 2
  fx_git -C "$dir" config tag.gpgsign false || return 2
  fx_git -C "$dir" config maintenance.auto false || return 2
  fx_git -C "$dir" config gc.auto 0 || return 2
  fx_git -C "$dir" config user.name moon-diagnosis-exec || return 2
  fx_git -C "$dir" config user.email moon-diagnosis-exec@example.invalid || return 2
  fx_git -C "$dir" add -A || return 2
  fx_git -C "$dir" commit -q -m one || return 2
  fx_git -C "$dir" rev-parse HEAD || return 2
  printf 'two\n' > "$dir/probe/file.txt"
  fx_git -C "$dir" add -A || return 2
  fx_git -C "$dir" commit -q -m two || return 2
}

# Every fixture git call ignores the host's global and system config (signing, hooks, identity).
fx_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git "$@"; }

outer_report_hash() {
  if [ -f "$REPO_ROOT/$REPORT_REL" ]; then cksum < "$REPO_ROOT/$REPORT_REL"; else echo absent; fi
}

# Unit 4. Runs the nested moon in DIR with an ALLOWLIST environment. Exits 2 on every
# infrastructure fault. Returns 0 when the fixture report is valid and shows the designed run.
run_nested_moon() {
  local dir="$1" base="$2" pin="$3" h_before h_after got nm_rc fx_rc
  got="$(cd "$dir" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$dir" moon --version 2>&1)" \
    || die_infra "'moon --version' failed in the fixture (pin $pin): $got"
  [ "$got" = "moon $pin" ] || die_infra "the fixture moon is '$got', expected 'moon $pin'"
  h_before="$(outer_report_hash)"
  nm_rc=0; (cd "$dir" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$dir" moon ci probe:fail3 probe:multi probe:missing probe:slow probe:ok --base "$base") >"$TMP/nested.out" 2>"$TMP/nested.err" || nm_rc=$?
  h_after="$(outer_report_hash)"
  [ "$h_before" = "$h_after" ] || die_infra "the nested run wrote to the real workspace report ($REPO_ROOT/$REPORT_REL)"
  [ "$nm_rc" -ne 0 ] || die_infra "the nested moon ci exited 0; the fixture did not fail, so the report proves nothing"
  if ! jq -e . "$dir/$REPORT_REL" >/dev/null 2>&1; then
    if grep -qF -- "proto-shim" "$TMP/nested.err"; then
      die_infra "no valid fixture report; the nested moon hit a proto-shim error (ci/CLAUDE.md, SMA-592): $(cat "$TMP/nested.err")"
    fi
    die_infra "no valid fixture report after the nested moon ci (rc=$nm_rc): $(cat "$TMP/nested.err")"
  fi
  # Spec M5: under inherited CI/GITHUB_* variables moon printed `Base revision: N/A` and ignored
  # --base. The allowlist removes them; this line proves it on every run.
  grep -qF -- "Base revision: $base" "$TMP/nested.out" || die_infra "the nested moon ci did not use --base $base (CI or GITHUB_* variables leaked?)"
  fx_rc=0; jq -e "$FIXTURE_RUN_CHECK" "$dir/$REPORT_REL" >/dev/null || fx_rc=$?
  [ "$fx_rc" -eq 0 ] || die_infra "the fixture did not run as designed (fixture-run check rc=$fx_rc); see $dir/$REPORT_REL"
}

# The gate's OWN jq check that the fixture ran as designed. Measured on moon 2.5.3, macOS and
# Linux: the slow task's ACTION status is `failed`; only its task-execution operation is
# `timed-out`.
FIXTURE_RUN_CHECK='[.actions[] | select((.label // "") | startswith("RunTask(probe:")) | "\(.label)=\(.status)"] | sort
  == ["RunTask(probe:fail3)=failed", "RunTask(probe:missing)=failed", "RunTask(probe:multi)=failed",
      "RunTask(probe:ok)=passed", "RunTask(probe:slow)=failed"]'

# Unit 5. Runs the documented query with jq directly. Prints nothing on success, or one row.
run_step1_query() {
  local pf="$1" ff="$2" dir="$3" out="$4" f q_rc
  local flags=()
  while IFS= read -r f; do flags+=("$f"); done < "$ff"
  q_rc=0; (cd "$dir" && jq ${flags[@]+"${flags[@]}"} "$(cat "$pf")" "$REPORT_REL") >"$out" 2>"$out.err" || q_rc=$?
  [ "$q_rc" -eq 0 ] || echo "query-error rc=$q_rc"
}

# Unit 6. One row per violation, nothing when clean. Rows are sorted.
STEP1_VERDICT_JQ='
def expected: {
  "RunTask(probe:fail3)":   {status: "failed",    exitCode: 3,    command: "exit 3"},
  "RunTask(probe:multi)":   {status: "failed",    exitCode: 4,    command: "echo a exit 4"},
  "RunTask(probe:missing)": {status: "failed",    exitCode: 127,  command: "no-such-binary-xyz"},
  "RunTask(probe:slow)":    {status: "timed-out", exitCode: null, command: "sleep 20"}
};
if length == 0 then "empty-output"
else
  . as $rows
  | [ ( expected | to_entries[] | .key as $l | .value as $want
        | ($rows | map(select(type == "object" and .label == $l))) as $m
        | if ($m | length) == 0 then "missing-row \($l)"
          else $m[]
            | (.exec | type) as $t
            | if $t != "array" then "bad-exec \($l) \(if has("exec") then $t else "absent" end)"
              elif any(.exec[]; type == "object" and .status == $want.status
                                and .exitCode == $want.exitCode and .command == $want.command)
              then empty
              else "wrong-exec \($l)" end
          end ),
      ( $rows[]
        | (if type == "object" then (.label // "<none>") else "<non-object>" end) as $l
        | select(expected | has($l) | not)
        | "unexpected-row \($l)" ) ]
  | sort | .[]
end'

step1_verdict() {
  local out="$1" v_rc
  v_rc=0; jq -r -s "$STEP1_VERDICT_JQ" "$out" >"$TMP/step1-verdict.rows" 2>"$TMP/step1-verdict.err" || v_rc=$?
  if [ "$v_rc" -ne 0 ]; then echo "verdict-error step1 rc=$v_rc"; return 0; fi
  cat "$TMP/step1-verdict.rows"
}

# Unit 7, Step 2a half. Integer microseconds: a float compare would red a difference of exactly
# 1000 ms. finishedAt is a naive UTC ISO time; lastRunTime is epoch milliseconds (spec M7).
STEP2A_PY='
import datetime, json, sys
d, rel = sys.argv[1], sys.argv[2]
with open(d + "/" + rel) as fh:
    rep = json.load(fh)
fin = {a.get("label"): a.get("finishedAt") for a in rep.get("actions", [])}
epoch = datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc)
for t in sys.argv[3:]:
    label = "RunTask(probe:" + t + ")"
    if not fin.get(label):
        print("step2a-no-action " + t)
        continue
    try:
        with open(d + "/.moon/cache/states/probe/" + t + "/lastRun.json") as fh:
            lr = int(json.load(fh)["lastRunTime"])
    except (OSError, ValueError, KeyError, TypeError):
        print("step2a-no-lastrun " + t)
        continue
    f = datetime.datetime.fromisoformat(fin[label]).replace(tzinfo=datetime.timezone.utc)
    diff_us = (f - epoch) // datetime.timedelta(microseconds=1) - lr * 1000
    if not 0 <= diff_us <= 1000000:
        print("step2a-out-of-range %s %.3f" % (t, diff_us / 1000))
'

step2_verdict() {
  local dir="$1" t f s="$1/.moon/cache/states/probe" p_rc
  for t in $FAILED_TASKS; do
    for f in stdout.log stderr.log lastRun.json; do
      [ -f "$s/$t/$f" ] || echo "step2-missing-file $t/$f"
    done
  done
  if [ -f "$s/missing/stderr.log" ] && ! grep -qF -- "command not found" "$s/missing/stderr.log"; then
    echo "step2-missing-no-command-not-found"
  fi
  if [ -f "$s/multi/stdout.log" ] && ! grep -qx -- "a" "$s/multi/stdout.log"; then
    echo "step2-multi-no-line-a"
  fi
  # shellcheck disable=SC2086 # FAILED_TASKS is a fixed list of four words.
  p_rc=0; python3 -c "$STEP2A_PY" "$dir" "$REPORT_REL" $FAILED_TASKS >"$TMP/step2a.rows" 2>"$TMP/step2a.err" || p_rc=$?
  if [ "$p_rc" -ne 0 ]; then echo "verdict-error step2 rc=$p_rc"; return 0; fi
  cat "$TMP/step2a.rows"
}

# Negative-control helper: writes a copy of MD in which the Step 1 fence body is BODY_FILE,
# indented like the fence. Prints ok, or a word when it cannot find the fence.
swap_step1_body() {
  local md="$1" body="$2" out="$3"
  awk -v bodyf="$body" '
    index($0, "<!-- moon-diagnosis:begin -->") { inblk = 1 }
    index($0, "<!-- moon-diagnosis:end -->")   { inblk = 0 }
    {
      if (inblk && state == 0 && index($0, "Step 1")) { state = 1; print; next }
      if (state == 1 && $0 ~ /^[ \t]*```bash[ \t]*$/) {
        print; match($0, /^[ \t]*/); ind = substr($0, 1, RLENGTH)
        while ((getline l < bodyf) > 0) print ind l
        state = 2; next
      }
      if (state == 2) { if ($0 ~ /^[ \t]*```[ \t]*$/) { state = 3; print }; next }
      print
    }
    END { if (state == 3) print "ok" > "/dev/stderr"; else print "no-fence" > "/dev/stderr" }
  ' "$md" > "$out" 2>"$out.verdict"
  cat "$out.verdict"
}

# ---------------------------------------------------------------------------------------------
# Self-test. Each case asserts the exact verdict text; the case count is asserted at the end.
ST_N=0
ST_FAIL=0
st_expect() {
  local name="$1" want="$2" got="$3"
  ST_N=$((ST_N + 1))
  if [ "$got" = "$want" ]; then
    printf 'PASS  [self-test] %s\n' "$name"
  else
    printf 'FAIL  [self-test] %s\n        want: %s\n        got:  %s\n' "$name" "$want" "$got"
    ST_FAIL=$((ST_FAIL + 1))
  fi
}

# Joins a verdict's rows with ";" so a case compares one string.
rows_of() { local r out=""; while IFS= read -r r; do out="${out:+$out;}$r"; done < "$1"; printf '%s' "$out"; }

st_md() {
  # $1 file, then lines
  local f="$1"; shift
  printf '%s\n' "$@" > "$f"
}

self_test() {
  local d="$TMP/st" q="$TMP/st/q" p="$TMP/st/p" o="$TMP/st/o" r="$TMP/st/r"
  mkdir -p "$d"
  local B='  <!-- moon-diagnosis:begin -->' E='  <!-- moon-diagnosis:end -->'
  local S1='  **Step 1 — which task, what command, what exit code.**'
  local S2='  **Step 2 — why.** `cat` the logs.'
  local FO='  ```bash' FC='  ```'

  # --- unit 1
  st_md "$d/md1" "x" "$B" "$S1" "$FO" "  jq '.a' \\" "     $REPORT_REL" "$FC" "  prose" "$S2" "$E"
  st_expect "u1 current shape" "ok" "$(extract_step1_query "$d/md1" "$q")"
  st_expect "u1 current shape dedented" "jq '.a' \\;   $REPORT_REL" "$(rows_of "$q")"
  st_md "$d/md2" "x" "$S1" "$FO" "  jq ." "$FC"
  st_expect "u1 no block" "no-block" "$(extract_step1_query "$d/md2" "$q")"
  st_md "$d/md3" "$B" "$S2" "$FO" "  jq ." "$FC" "$E"
  st_expect "u1 no Step 1" "no-step1" "$(extract_step1_query "$d/md3" "$q")"
  st_md "$d/md4" "$B" "$S1" "  prose only" "$S2" "$FO" "  jq ." "$FC" "$E"
  st_expect "u1 Step 1 with no fence" "no-fence" "$(extract_step1_query "$d/md4" "$q")"
  st_md "$d/md5" "$B" "$S1" "$FO" "  jq ." "$E"
  st_expect "u1 unclosed fence" "unclosed-fence" "$(extract_step1_query "$d/md5" "$q")"
  st_md "$d/md6" "$B" "$S1" "$FO" "  jq ." "$FC" "$FO" "  jq .b" "$FC" "$S2" "$E"
  st_expect "u1 two Step 1 fences" "multiple-step1" "$(extract_step1_query "$d/md6" "$q")"
  st_md "$d/md7" "$B" "$S1" "$FO" "  jq ." "$FC" "$S2" "$S1" "$E"
  st_expect "u1 second Step 1 heading" "multiple-step1" "$(extract_step1_query "$d/md7" "$q")"
  st_md "$d/md8" "$B" "$B" "$S1" "$FO" "  jq ." "$FC" "$E"
  st_expect "u1 two begin markers" "no-block" "$(extract_step1_query "$d/md8" "$q")"

  # --- unit 2
  printf '%s\n' "jq '.actions[] | select(.status==\"failed\")" \
    "    | {label, error," \
    "       exec: [.operations[] | select(.meta.type==\"task-execution\")" \
    "              | {status, exitCode: .meta.exitCode, command: .meta.command}]}' \\" \
    "   $REPORT_REL" > "$q"
  st_expect "u2 current query" "ok" "$(query_shape_verdict "$q" "$p")"
  st_expect "u2 current query flags" "" "$(cat "$p.flags")"
  printf '%s\n' "${OLD_STEP1_LINES[@]}" > "$q"
  st_expect "u2 pre-SMA-711 query" "ok" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq -r -c '.a' $REPORT_REL" > "$q"
  st_expect "u2 allowed flags" "ok" "$(query_shape_verdict "$q" "$p")"
  st_expect "u2 allowed flags file" "-r;-c" "$(rows_of "$p.flags")"
  printf '%s\n' "jq '.a' $REPORT_REL | head" > "$q"
  st_expect "u2 pipe after path" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq '.a' $REPORT_REL > out" > "$q"
  st_expect "u2 redirect after path" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq --arg x y '.a' $REPORT_REL" > "$q"
  st_expect "u2 flag not allowed" "bad-shape flag-not-allowed --arg" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq '.a' .moon/cache/runReport.json" > "$q"
  st_expect "u2 another file" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq '.a' '.b' $REPORT_REL" > "$q"
  st_expect "u2 quote in program" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "cat $REPORT_REL" > "$q"
  st_expect "u2 not jq" "bad-shape not-jq" "$(query_shape_verdict "$q" "$p")"

  # --- unit 6
  printf '%s\n' \
    '{"label":"RunTask(probe:fail3)","error":"e","exec":[{"status":"failed","exitCode":3,"command":"exit 3"}]}' \
    '{"label":"RunTask(probe:missing)","error":"e","exec":[{"status":"failed","exitCode":127,"command":"no-such-binary-xyz"}]}' \
    '{"label":"RunTask(probe:multi)","error":"e","exec":[{"status":"failed","exitCode":4,"command":"echo a exit 4"}]}' \
    '{"label":"RunTask(probe:slow)","error":"e","exec":[{"status":"timed-out","exitCode":null,"command":"sleep 20"}]}' > "$o.good"
  step1_verdict "$o.good" > "$r"
  st_expect "u6 correct output" "" "$(rows_of "$r")"
  printf '%s\n' \
    '{"label":"RunTask(probe:fail3)","error":"e","exec":{"command":null,"exitCode":null}}' \
    '{"label":"RunTask(probe:missing)","error":"e","exec":{"command":null,"exitCode":null}}' \
    '{"label":"RunTask(probe:multi)","error":"e","exec":{"command":null,"exitCode":null}}' \
    '{"label":"RunTask(probe:slow)","error":"e","exec":{"command":null,"exitCode":null}}' > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 old null output" "bad-exec RunTask(probe:fail3) object;bad-exec RunTask(probe:missing) object;bad-exec RunTask(probe:multi) object;bad-exec RunTask(probe:slow) object" "$(rows_of "$r")"
  : > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 empty output" "empty-output" "$(rows_of "$r")"
  { cat "$o.good"; printf '%s\n' '{"label":"RunTask(probe:ok)","exec":[]}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 ok row present" "unexpected-row RunTask(probe:ok)" "$(rows_of "$r")"
  grep -vF 'probe:multi' "$o.good" > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 one row missing" "missing-row RunTask(probe:multi)" "$(rows_of "$r")"
  { grep -vF 'probe:slow' "$o.good"; printf '%s\n' '{"label":"RunTask(probe:slow)","exec":null}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 exec null" "bad-exec RunTask(probe:slow) null" "$(rows_of "$r")"
  { grep -vF 'probe:slow' "$o.good"; printf '%s\n' '{"label":"RunTask(probe:slow)"}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 exec absent" "bad-exec RunTask(probe:slow) absent" "$(rows_of "$r")"
  { grep -vF 'probe:fail3' "$o.good"; printf '%s\n' '{"label":"RunTask(probe:fail3)","exec":[{"exitCode":3,"command":"exit 3"}]}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 status dropped" "wrong-exec RunTask(probe:fail3)" "$(rows_of "$r")"
  printf '%s\n' 'this is not json' > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 verdict tool fails" "verdict-error step1 rc=N" "$(rows_of "$r" | sed 's/rc=[1-9][0-9]*$/rc=N/')"

  # --- unit 7 (fixture report and state files; finishedAt 2026-09-27T14:49:28.972000 is
  # 1790520568972000 us after the epoch)
  local diff want
  for diff in 0 1000 1001 -1; do
    st_step2_fixture "$d/s2" "$((1790520568972 - diff))"
    case "$diff" in
      0|1000) want="" ;;
      *) want="step2a-out-of-range fail3 $diff.000;step2a-out-of-range multi $diff.000;step2a-out-of-range missing $diff.000;step2a-out-of-range slow $diff.000" ;;
    esac
    step2_verdict "$d/s2" > "$r"
    st_expect "u7 Step 2a difference $diff ms" "$want" "$(rows_of "$r")"
  done
  st_step2_fixture "$d/s2" 1790520568972
  rm -f "$d/s2/.moon/cache/states/probe/multi/stderr.log"
  step2_verdict "$d/s2" > "$r"
  st_expect "u7 missing stderr.log" "step2-missing-file multi/stderr.log" "$(rows_of "$r")"
  st_step2_fixture "$d/s2" 1790520568972
  printf 'something else\n' > "$d/s2/.moon/cache/states/probe/missing/stderr.log"
  step2_verdict "$d/s2" > "$r"
  st_expect "u7 no command not found" "step2-missing-no-command-not-found" "$(rows_of "$r")"

  # --- negative-control helper
  printf '%s\n' "${OLD_STEP1_LINES[@]}" > "$d/old"
  st_expect "swap Step 1 body" "ok" "$(swap_step1_body "$d/md1" "$d/old" "$d/md1.swapped")"
  extract_step1_query "$d/md1.swapped" "$q" > /dev/null
  st_expect "swap round-trips through unit 1" "$(rows_of "$d/old")" "$(rows_of "$q")"

  local n="$ST_N"
  st_expect "self-test case count" "$SELF_TEST_CASES" "$n"
  if [ "$ST_FAIL" -ne 0 ]; then
    printf 'moon-diagnosis-exec self-test: %d case(s) FAILED\n' "$ST_FAIL" >&2
    return 1
  fi
  printf 'moon-diagnosis-exec self-test: all %d cases passed\n' "$ST_N"
}

# Writes a Step 2 fixture under DIR: a report with finishedAt 2026-09-27T14:49:28.972000 for each
# failed task, and state files whose lastRun.json holds LASTRUN.
st_step2_fixture() {
  local dir="$1" lastrun="$2" t
  rm -rf "$dir"
  mkdir -p "$dir/.moon/cache/states/probe"
  printf '{"actions":[%s]}\n' \
    '{"label":"RunTask(probe:fail3)","finishedAt":"2026-09-27T14:49:28.972000"},{"label":"RunTask(probe:multi)","finishedAt":"2026-09-27T14:49:28.972000"},{"label":"RunTask(probe:missing)","finishedAt":"2026-09-27T14:49:28.972000"},{"label":"RunTask(probe:slow)","finishedAt":"2026-09-27T14:49:28.972000"}' \
    > "$dir/$REPORT_REL"
  for t in $FAILED_TASKS; do
    mkdir -p "$dir/.moon/cache/states/probe/$t"
    : > "$dir/.moon/cache/states/probe/$t/stdout.log"
    : > "$dir/.moon/cache/states/probe/$t/stderr.log"
    printf '{"exitCode":1,"lastRunTime":%s}\n' "$lastrun" > "$dir/.moon/cache/states/probe/$t/lastRun.json"
  done
  printf 'a\n' > "$dir/.moon/cache/states/probe/multi/stdout.log"
  printf 'bash: line 1: no-such-binary-xyz: command not found\n' > "$dir/.moon/cache/states/probe/missing/stderr.log"
}

# ---------------------------------------------------------------------------------------------
ROWS=0
emit() { printf 'ROW %s\n' "$1"; ROWS=$((ROWS + 1)); }
emit_file() { local row; while IFS= read -r row; do [ -n "$row" ] && emit "$row"; done < "$1"; }

preflight() {
  local tool
  for tool in moon jq git python3; do
    command -v "$tool" >/dev/null 2>&1 || die_infra "'$tool' is not on PATH"
  done
}

repo_moon_pin() {
  local pin
  pin="$(sed -n 's/^moon = "\([0-9][0-9.]*\)"$/\1/p' "$REPO_ROOT/.prototools")" || die_infra "cannot read .prototools"
  case "$pin" in
    ''|*[!0-9.]*) die_infra "expected one 'moon = \"X.Y.Z\"' pin in .prototools, got: ${pin:-<none>}" ;;
  esac
  printf '%s' "$pin"
}

real_run() {
  local v pin base fx="$TMP/fixture"
  preflight
  v="$(extract_step1_query "$CLAUDE_MD" "$TMP/query")" || die_infra "extract_step1_query crashed"
  if [ "$v" != "ok" ]; then emit "extract $v"; return 1; fi
  v="$(query_shape_verdict "$TMP/query" "$TMP/prog")" || die_infra "query_shape_verdict crashed"
  if [ "$v" != "ok" ]; then emit "$v"; return 1; fi
  pin="$(repo_moon_pin)"
  base="$(make_fixture "$fx" "$pin")" || die_infra "make_fixture failed"
  run_nested_moon "$fx" "$base" "$pin"
  run_step1_query "$TMP/prog" "$TMP/prog.flags" "$fx" "$TMP/step1.out" > "$TMP/step1-query.rows" || die_infra "run_step1_query crashed"
  emit_file "$TMP/step1-query.rows"
  if [ "$ROWS" -eq 0 ]; then
    step1_verdict "$TMP/step1.out" > "$TMP/step1.rows" || die_infra "step1_verdict crashed"
    emit_file "$TMP/step1.rows"
  fi
  step2_verdict "$fx" > "$TMP/step2.rows" || die_infra "step2_verdict crashed"
  emit_file "$TMP/step2.rows"
  if [ "$ROWS" -ne 0 ]; then
    printf 'moon-diagnosis-exec: %d row(s) FAILED; the documented procedure does not match a real report\n' "$ROWS" >&2
    return 1
  fi
  printf 'moon-diagnosis-exec: Step 1, Step 2 and Step 2a hold against a real failed moon ci report (moon %s)\n' "$pin"
}

negative_control() {
  local ctl_md="$TMP/ctl-CLAUDE.md" ctl_out="$TMP/ctl.out" v nc_rc
  printf '%s\n' "${OLD_STEP1_LINES[@]}" > "$TMP/old-query"
  v="$(swap_step1_body "$CLAUDE_MD" "$TMP/old-query" "$ctl_md")"
  [ "$v" = "ok" ] || die_infra "negative control: cannot find the Step 1 fence in $CLAUDE_MD ($v)"
  nc_rc=0; "$BASH" "$HERE/run.sh" --claude-md "$ctl_md" >"$ctl_out" 2>&1 || nc_rc=$?
  sed -n 's/^ROW //p' "$ctl_out" | LC_ALL=C sort > "$TMP/ctl.rows"
  printf '%s\n' "${NEGATIVE_WANT_ROWS[@]}" | LC_ALL=C sort > "$TMP/ctl.want"
  case "$nc_rc" in
    0) printf 'moon-diagnosis-exec negative control: FAILED, the gate passed the pre-SMA-711 query\n' >&2; return 1 ;;
    1) ;;
    *) printf 'moon-diagnosis-exec negative control: infrastructure error (rc=2): the child exited %s\n' "$nc_rc" >&2; cat "$ctl_out" >&2; return 2 ;;
  esac
  if ! cmp -s "$TMP/ctl.rows" "$TMP/ctl.want"; then
    printf 'moon-diagnosis-exec negative control: FAILED, the row set differs from the expected set\n' >&2
    printf 'want:\n' >&2; cat "$TMP/ctl.want" >&2
    printf 'got:\n' >&2; cat "$TMP/ctl.rows" >&2
    return 1
  fi
  printf 'moon-diagnosis-exec negative control: passed, the pre-SMA-711 query reds with exactly %d rows\n' "${#NEGATIVE_WANT_ROWS[@]}"
}

if [ "$SELFTEST" = 1 ]; then
  st_rc=0; self_test || st_rc=$?
  exit "$st_rc"
fi
if [ "$NEGATIVE" = 1 ]; then
  nc_rc=0; negative_control || nc_rc=$?
  exit "$nc_rc"
fi
real_rc=0; real_run || real_rc=$?
exit "$real_rc"
````

Then make it executable: `chmod +x ci/moon-diagnosis/run.sh`.

- [ ] **Step 3: Run the self-test under system bash 3.2**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/moon-diagnosis/run.sh --self-test; echo rc=$?
```
Expected: 38 `PASS  [self-test]` lines, the last line `moon-diagnosis-exec self-test: all 38 cases passed`, and `rc=0`.

- [ ] **Step 4: Run the self-test under Homebrew bash 5 with an alarm (Review Focus 5)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && perl -e 'alarm 120; exec @ARGV' /opt/homebrew/bin/bash ci/moon-diagnosis/run.sh --self-test > "$TMPDIR/sma714-st5.out" 2>&1; echo "rc=$?"; tail -n 1 "$TMPDIR/sma714-st5.out"
```
(The Bash tool runs zsh, so this plan avoids `PIPESTATUS` and captures to a file instead.)
Expected: `all 38 cases passed` and `rc=0`. An `rc=142` (SIGALRM) is a hang: stop and report it as a finding, do not continue.

- [ ] **Step 5: Run the real mode and the negative control**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && time /bin/bash ci/moon-diagnosis/run.sh; echo rc=$?; time /bin/bash ci/moon-diagnosis/run.sh --negative-control; echo rc=$?
```
Expected: `moon-diagnosis-exec: Step 1, Step 2 and Step 2a hold against a real failed moon ci report (moon 2.5.3)` with `rc=0`, then `moon-diagnosis-exec negative control: passed, the pre-SMA-711 query reds with exactly 4 rows` with `rc=0`. Each takes about 7 s.

- [ ] **Step 6: Scan the script with check 13's own reader rule**

Run (this extracts check 13's definitions from `ci/actionlint/run.sh` into the scratchpad and runs them on the new file only):
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && S="$TMPDIR/sma714-ee" && mkdir -p "$S" && a="$(grep -n '^EARLY_EXIT_READER_ALLOWED=(' ci/actionlint/run.sh | cut -d: -f1)" && b="$(grep -n '^early_exit_reader_verdict()' ci/actionlint/run.sh | cut -d: -f1)" && sed -n "${a},$((b - 1))p" ci/actionlint/run.sh > "$S/ee.sh" && printf '%s\n' ci/moon-diagnosis/run.sh > "$S/list" && /bin/bash -c ". '$S/ee.sh'; early_exit_reader_rows '$S/list'"; echo rc=$?
```
Expected: no output, and `rc=0`.

- [ ] **Step 7: Write `ci/moon-diagnosis/README.md` with exactly this content**

````markdown
<!-- SPDX-License-Identifier: Apache-2.0 -->
<!-- moon-diagnosis:ok -->

# `repo:moon-diagnosis-exec`

This gate runs the root `CLAUDE.md` procedure "Diagnosing an unattributed `moon ci` failure"
against a real failed `moon ci` report (SMA-714). Check 12 of `repo:actionlint` gates only the
presence of five literals in that block. This gate runs the Step 1 query and checks the Step 2
and Step 2a facts.

Spec: `docs/superpowers/specs/2026-09-28-sma-714-moon-diagnosis-exec-gate-design.md`.

## Why it exists

SMA-597 wrote the Step 1 `jq` query with `{command, exitCode}`. Moon writes these two fields
under `.meta`, so the query printed `null` for both. Every gate stayed green until SMA-711 fixed
the query. Without this gate, an edit can break the query again with no red.

## What it does

1. It reads the text between the two `moon-diagnosis` markers of the root `CLAUDE.md`. It finds
   the `Step 1` heading and the `bash` fence after it, and takes the fence body as the query.
2. It checks the shape of the query: the word `jq`, zero or more of the flags `-r`, `-c`, `-s`,
   `-e`, one single-quoted program, then the path `.moon/cache/ciReport.json` and nothing else.
   The gate never runs doc text through `bash`. It calls `jq` directly with the program.
3. It makes a throwaway git workspace under a `mktemp -d` directory. The workspace pins the moon
   version of the repo `.prototools` and has five tasks: `fail3` (`exit 3`), `multi` (a two-line
   script that ends with `exit 4`), `missing` (a command that does not exist), `slow`
   (`sleep 20` with a 2 s timeout) and `ok` (`true`). Every fixture `git` call ignores the host's
   global and system git config, so the host's commit signing never applies.
4. It runs `moon ci` in that workspace with an allowlist environment (`env -i` and seven named
   variables). An inherited `MOON_WORKSPACE_ROOT` would move the run to the real workspace, and
   an inherited `PROTO_MOON_VERSION` would override the fixture pin. See `ci/CLAUDE.md`, section
   "A nested `moon` call that targets a fixture workspace".
5. It proves that the fixture ran as designed, with its own `jq` check: four `failed` task
   actions and one `passed` action. It also proves that moon used `--base`.
6. It runs the documented query and compares each output row with the expected
   `{status, exitCode, command}` values.
7. It checks the state files of each failed task (Step 2) and that
   `0 <= finishedAt_ms - lastRunTime <= 1000` (Step 2a).

## Modes

- `run.sh`: the real run on the working-tree `CLAUDE.md`.
- `run.sh --self-test`: fixture tables for the pure units, with no moon and no git. Each case
  asserts the exact verdict text, and the table asserts its own case count.
- `run.sh --negative-control`: a copy of `CLAUDE.md` in which the Step 1 query is the pre-SMA-711
  query. The full real path must fail with exactly four `bad-exec <label> object` rows.

## Exit codes

- `0`: the procedure holds.
- `1`: an assertion failed. Each row is printed as `ROW <row>`.
- `2`: an infrastructure error. The message holds `infrastructure error (rc=2)`. Examples: no
  `moon`, `jq`, `git` or `python3`; a fixture moon version that differs from the pin; no fixture
  report; a nested run that changed the real workspace report; a fixture that did not run as
  designed.

## Registration

The gate is registered in the seven places that `ci/CLAUDE.md` ("Registering a new `repo:*`
gate") lists. `MOON_DIAGNOSIS_SH_CALL_SITES` in `ci/affected-graph/ci_targets.py` pins 59 lines of
`run.sh` as whole lines. If you change a pinned line, change the pin in the same commit.

## Limits

- The gate runs only when `CLAUDE.md`, `.prototools` or `ci/moon-diagnosis/**` changes. A moon
  upgrade that arrives by another path does not schedule it.
- It checks Step 1, Step 2 and Step 2a only. Step 0, Step 3, the "What cannot work" paragraph and
  the CI note are prose or commands that change state. No gate runs them.
- It reads only the root `CLAUDE.md` block. Diagnosis advice in other files is not checked
  (`ci/actionlint/README.md` L32).
- The nested run models a LOCAL run, because the procedure is for local runs. The CI-mode
  behaviour of `--base` is not checked.
- `jq` comes from the runner image in CI and from the host locally. It is not pinned.
- The Step 2a bound of 1000 ms rests on macOS and Linux samples with a maximum of 15.6 ms. If a
  false red occurs, measure the difference again before you change the bound.
- The nested `moon ci` can abort with the proto-shim error that `ci/CLAUDE.md` records for
  `repo:affected-smoke`. The gate reports it as rc 2.
````

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && git branch --show-current && git add ci/moon-diagnosis/run.sh ci/moon-diagnosis/README.md && git commit -m "feat(ci): add the moon-diagnosis-exec gate script (SMA-714)

The gate runs the Step 1 query of the root CLAUDE.md moon-diagnosis
block against a real failed moon ci report in a fixture workspace,
and checks the Step 2 and Step 2a facts that the procedure uses.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
The branch line must print `feature/sma-714-moon-diagnosis-exec-gate`. Do not write a `#NNN` or a `token: value` line in the commit body (commitlint `footer-leading-blank`).

---

### Task 2: The moon task and the seven registrations

All seven obligations change in ONE commit, because `repo:affected-smoke` reds on a self-test
task that is not registered (`check_self_scheduled_coverage`), and on a `T` entry with no
matching task.

**Files:**
- Modify: `moon.yml` (append the task after line 1025; add one input to `affected-smoke` after line 257)
- Modify: `.github/workflows/ci.yml:267`
- Modify: `CLAUDE.md:146`
- Modify: `ci/actionlint/run.sh:2160-2164`
- Modify: `ci/affected-graph/ci_targets.py` (with the edit script below)

**Interfaces:**
- Consumes: Task 1's `ci/moon-diagnosis/run.sh`, the 59 lines listed in `MOON_DIAGNOSIS_SH_CALL_SITES` below (each must occur exactly once as a stripped line).
- Produces: `repo:moon-diagnosis-exec`; `check_self_invocation(run_sh_text, scripts, actionlint_sh_text, release_parity_sh_text, workflow_credentials_sh_text, release_plan_sh_text, ruff_sh_text, next_public_free_sh_text, helm_render_sh_text, moon_diagnosis_sh_text)` (ten REQUIRED positional parameters); `_CALL_SITE_SOURCE_KEYS` gains `"moon_diagnosis"`.

- [ ] **Step 1: Append the moon task to the end of `moon.yml`**

Add these lines after the last line of the file (`      - 'ts/packages/paigasus-proto/src/capability.ts'`, the end of the `helm-render` task), with one blank line before them:

```yaml

  moon-diagnosis-exec:
    description: 'Run the CLAUDE.md moon-diagnosis Step 1 query against a real failed moon ci report (SMA-714).'
    # WHY THIS EXISTS — check 12 of repo:actionlint gates only the PRESENCE of five literals in the
    # root CLAUDE.md moon-diagnosis block. SMA-597 shipped a Step 1 query that printed null for
    # every value, and every gate stayed green until SMA-711 fixed it. This task runs the
    # documented query, and checks Steps 2 and 2a, against a real failed `moon ci` in a fixture
    # workspace (spec 2026-09-28-sma-714-moon-diagnosis-exec-gate-design.md).
    #
    # INPUTS. CLAUDE.md holds the procedure. .prototools holds the moon pin: a moon bump is the
    # event most likely to move a report field. SELF_TASK_EXPECTED_GLOBS pins these three.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block. `set -euo pipefail`
    # is REQUIRED: Moon takes a `script:` block's status from its LAST command, so without it a
    # failing control is masked by the passing real run. SELF_SCHEDULED_GATES pins these four lines.
    script: |
      set -euo pipefail
      bash ci/moon-diagnosis/run.sh --self-test
      bash ci/moon-diagnosis/run.sh --negative-control
      bash ci/moon-diagnosis/run.sh
    toolchain: 'system'
    inputs:
      - 'CLAUDE.md'
      - '.prototools'
      - 'ci/moon-diagnosis/**/*'
```

- [ ] **Step 2: Add the reachability input to `affected-smoke` in `moon.yml`**

Directly after the line `      - 'ci/helm-render/**/*'` at line 257 (the one that follows the
`SMA-513 PR 2b — this task pins load-bearing lines inside ci/helm-render/run.sh` comment), and
before the `# SMA-541` comment, insert:

```yaml
      # SMA-714 — this task pins load-bearing lines inside ci/moon-diagnosis/run.sh
      # (MOON_DIAGNOSIS_SH_CALL_SITES), so a change under ci/moon-diagnosis/ MUST re-key it. The
      # broad 'ci/**/*' entry already covers it for SCHEDULING; the narrow entry is kept because
      # check 8e in ci/actionlint/run.sh floors this array's length and the file's own policy is to
      # keep narrow globs rather than collapse them into the broad one.
      - 'ci/moon-diagnosis/**/*'
```
Check that `'ci/helm-render/**/*'` occurs twice in `moon.yml` (the affected-smoke input and the helm-render task input) and that you edited the FIRST one (line 257): `grep -n "ci/helm-render/\*\*/\*" moon.yml`.

- [ ] **Step 3: Floor the input in `ci/actionlint/run.sh`**

In `T_AFFECTED_SMOKE_REQUIRED_INPUTS`, directly after `  'ci/helm-render/**/*'` (line 2163) and before `  'CLAUDE.md'`, insert:

```bash
  # SMA-714 — floors the input that makes MOON_DIAGNOSIS_SH_CALL_SITES reachable. Without it, a
  # PR editing ci/moon-diagnosis/** does not schedule repo:affected-smoke, and neither the
  # SELF_SCHEDULED_GATES nor the SELF_TASK_EXPECTED_GLOBS pin for that gate can fire.
  'ci/moon-diagnosis/**/*'
```
The array floor (`-ge 20` at line 5778) needs no change.

- [ ] **Step 4: Add the target to `ci.yml` and to the `ci-targets` block**

In `.github/workflows/ci.yml` line 267, replace `:helm-render :test-e2e)` with `:helm-render :moon-diagnosis-exec :test-e2e)`. Do not break the line: `T` must stay a single-line bash array (SMA-541).

In the root `CLAUDE.md` line 146, replace the line
```
  :next-public-free :helm-render :test-e2e
```
with
```
  :next-public-free :helm-render :moon-diagnosis-exec :test-e2e
```
Do not touch the two marker lines around the block. Run `grep -c 'ci-targets:begin' CLAUDE.md; grep -c 'ci-targets:end' CLAUDE.md` and expect `1` and `1`.

- [ ] **Step 5: Save the pin tuple and the edit script in the scratchpad**

Save this text as `$TMPDIR/sma714/pins_tuple.py.txt` (it is inserted verbatim into `ci_targets.py`):

```python
# SMA-714 — ci/moon-diagnosis/run.sh's load-bearing lines. REACHABILITY IS NOT AUTOMATIC: this
# check only runs when repo:affected-smoke is scheduled, so moon.yml lists `ci/moon-diagnosis/**/*`
# among its inputs and ci/actionlint/run.sh's T_AFFECTED_SMOKE_REQUIRED_INPUTS floors that entry.
#
# Matched as stripped WHOLE LINES, like the helm-render haystack and for both of its reasons: the
# real lines are indented inside functions and `case` arms, and a COMMENTED-OUT copy must not
# satisfy a pin. Every entry occurs EXACTLY ONCE in run.sh. No entry holds the token that check 12
# of repo:actionlint scans for, so this file needs no check-12 marker.
#
# The spec (§6.5) pins every call that carries a check AND its rc guard: the workflow-credentials
# measurement showed that pins on report lines alone let a control assert nothing.
MOON_DIAGNOSIS_SH_CALL_SITES = (
    # The flag parse (both modes and the hidden --claude-md argument) and the three dispatch arms.
    '--self-test)        SELFTEST=1; shift ;;',
    '--negative-control) NEGATIVE=1; shift ;;',
    '--claude-md)        [ $# -ge 2 ] || die_infra "--claude-md needs a path"; CLAUDE_MD="$2"; shift 2 ;;',
    'if [ "$SELFTEST" = 1 ]; then',
    'st_rc=0; self_test || st_rc=$?',
    'exit "$st_rc"',
    'if [ "$NEGATIVE" = 1 ]; then',
    'nc_rc=0; negative_control || nc_rc=$?',
    'exit "$nc_rc"',
    'real_rc=0; real_run || real_rc=$?',
    'exit "$real_rc"',
    # The self-test case count, its assertion and the failure guard.
    'SELF_TEST_CASES=37',
    'local n="$ST_N"',
    'st_expect "self-test case count" "$SELF_TEST_CASES" "$n"',
    'if [ "$ST_FAIL" -ne 0 ]; then',
    # The real path: each unit call and its rc guard, and the row report.
    'v="$(extract_step1_query "$CLAUDE_MD" "$TMP/query")" || die_infra "extract_step1_query crashed"',
    'if [ "$v" != "ok" ]; then emit "extract $v"; return 1; fi',
    'v="$(query_shape_verdict "$TMP/query" "$TMP/prog")" || die_infra "query_shape_verdict crashed"',
    'if [ "$v" != "ok" ]; then emit "$v"; return 1; fi',
    'run_nested_moon "$fx" "$base" "$pin"',
    'run_step1_query "$TMP/prog" "$TMP/prog.flags" "$fx" "$TMP/step1.out" > "$TMP/step1-query.rows" || die_infra "run_step1_query crashed"',
    'emit_file "$TMP/step1-query.rows"',
    'if [ "$ROWS" -eq 0 ]; then',
    'step1_verdict "$TMP/step1.out" > "$TMP/step1.rows" || die_infra "step1_verdict crashed"',
    'emit_file "$TMP/step1.rows"',
    'step2_verdict "$fx" > "$TMP/step2.rows" || die_infra "step2_verdict crashed"',
    'emit_file "$TMP/step2.rows"',
    'if [ "$ROWS" -ne 0 ]; then',
    # Unit 4: the moon --version check, the env -i allowlist lines, the outer report hash compare,
    # the nested rc guard, the report validity check, the --base check and the fixture-run check.
    'got="$(cd "$dir" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$dir" moon --version 2>&1)" \\',
    '[ "$got" = "moon $pin" ] || die_infra "the fixture moon is \'$got\', expected \'moon $pin\'"',
    'h_before="$(outer_report_hash)"',
    'nm_rc=0; (cd "$dir" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$dir" moon ci probe:fail3 probe:multi probe:missing probe:slow probe:ok --base "$base") >"$TMP/nested.out" 2>"$TMP/nested.err" || nm_rc=$?',
    'h_after="$(outer_report_hash)"',
    '[ "$h_before" = "$h_after" ] || die_infra "the nested run wrote to the real workspace report ($REPO_ROOT/$REPORT_REL)"',
    '[ "$nm_rc" -ne 0 ] || die_infra "the nested moon ci exited 0; the fixture did not fail, so the report proves nothing"',
    'if ! jq -e . "$dir/$REPORT_REL" >/dev/null 2>&1; then',
    'grep -qF -- "Base revision: $base" "$TMP/nested.out" || die_infra "the nested moon ci did not use --base $base (CI or GITHUB_* variables leaked?)"',
    'fx_rc=0; jq -e "$FIXTURE_RUN_CHECK" "$dir/$REPORT_REL" >/dev/null || fx_rc=$?',
    '[ "$fx_rc" -eq 0 ] || die_infra "the fixture did not run as designed (fixture-run check rc=$fx_rc); see $dir/$REPORT_REL"',
    # Units 5, 6 and 7: the query rc guard, each verdict tool's rc route, and the Step 2a bound.
    'q_rc=0; (cd "$dir" && jq ${flags[@]+"${flags[@]}"} "$(cat "$pf")" "$REPORT_REL") >"$out" 2>"$out.err" || q_rc=$?',
    '[ "$q_rc" -eq 0 ] || echo "query-error rc=$q_rc"',
    'v_rc=0; jq -r -s "$STEP1_VERDICT_JQ" "$out" >"$TMP/step1-verdict.rows" 2>"$TMP/step1-verdict.err" || v_rc=$?',
    'if [ "$v_rc" -ne 0 ]; then echo "verdict-error step1 rc=$v_rc"; return 0; fi',
    'p_rc=0; python3 -c "$STEP2A_PY" "$dir" "$REPORT_REL" $FAILED_TASKS >"$TMP/step2a.rows" 2>"$TMP/step2a.err" || p_rc=$?',
    'if [ "$p_rc" -ne 0 ]; then echo "verdict-error step2 rc=$p_rc"; return 0; fi',
    'if not 0 <= diff_us <= 1000000:',
    # The negative control: the swap, the child call, the row capture, the rc map, the exact
    # row-set compare and the four expected rows.
    'v="$(swap_step1_body "$CLAUDE_MD" "$TMP/old-query" "$ctl_md")"',
    '[ "$v" = "ok" ] || die_infra "negative control: cannot find the Step 1 fence in $CLAUDE_MD ($v)"',
    'nc_rc=0; "$BASH" "$HERE/run.sh" --claude-md "$ctl_md" >"$ctl_out" 2>&1 || nc_rc=$?',
    'sed -n \'s/^ROW //p\' "$ctl_out" | LC_ALL=C sort > "$TMP/ctl.rows"',
    'printf \'%s\\n\' "${NEGATIVE_WANT_ROWS[@]}" | LC_ALL=C sort > "$TMP/ctl.want"',
    "0) printf 'moon-diagnosis-exec negative control: FAILED, the gate passed the pre-SMA-711 query\\n' >&2; return 1 ;;",
    '1) ;;',
    '*) printf \'moon-diagnosis-exec negative control: infrastructure error (rc=2): the child exited %s\\n\' "$nc_rc" >&2; cat "$ctl_out" >&2; return 2 ;;',
    'if ! cmp -s "$TMP/ctl.rows" "$TMP/ctl.want"; then',
    '"bad-exec RunTask(probe:fail3) object"',
    '"bad-exec RunTask(probe:missing) object"',
    '"bad-exec RunTask(probe:multi) object"',
    '"bad-exec RunTask(probe:slow) object"',
)
```

Save this script as `$TMPDIR/sma714/sma714_ci_targets_patch.py`. It is a one-off edit tool. Do NOT commit it. Every edit asserts that its anchor occurs exactly once, and step 13 asserts that it rewrote exactly 62 `check_self_invocation(...)` calls inside `self_test()`:

```python
"""One-off edit script for SMA-714 Task 2. Not committed. Usage: python3 <this> <path/to/ci_targets.py>.

Every edit is anchored on exact text and asserts that its anchor occurs exactly once, so a drifted
base fails loudly instead of editing the wrong place.
"""
import re
import sys

path = sys.argv[1]
src = open(path, encoding="utf-8").read()


def once(old, new):
    global src
    n = src.count(old)
    assert n == 1, f"anchor occurs {n} times, expected 1:\n{old}"
    src = src.replace(old, new)


# 1. REQUIRED_REPO_TASKS (obligation 6).
once(
    '    "helm-render",\n)\n\n# SMA-553 D13',
    '    "helm-render",\n'
    "    # SMA-714. Same reasoning as the entries above: repo:moon-diagnosis-exec carries a\n"
    "    # --negative-control, and check_forward's `want`/`got` shrink CONSISTENTLY when a task is\n"
    "    # dropped from `T` and made CI-ineligible in the same edit — so without a floor entry the whole\n"
    "    # gate, control included, could be switched off with every check green.\n"
    '    "moon-diagnosis-exec",\n)\n\n# SMA-553 D13',
)

# 2. SELF_TASK_EXPECTED_GLOBS (obligation 4). Globs first, then files; `.prototools` sorts before
# `CLAUDE.md` because `.` < `C`.
once(
    '        "ts/packages/paigasus-proto/src/capability.ts",\n    ),\n}\n',
    '        "ts/packages/paigasus-proto/src/capability.ts",\n    ),\n'
    "    # SMA-714. CLAUDE.md holds the procedure the gate runs, and .prototools holds the moon pin,\n"
    "    # the event most likely to move a report field. Drop either and the gate serves a cached\n"
    "    # PASS on the exact edit it exists to catch.\n"
    '    "moon-diagnosis-exec": (\n'
    '        "ci/moon-diagnosis/**/*",\n'
    '        ".prototools",\n'
    '        "CLAUDE.md",\n'
    "    ),\n}\n",
)

# 3. SELF_SCHEDULED_GATES (obligation 3).
once(
    '        "bash ci/helm-render/run.sh",\n    ),\n}\n',
    '        "bash ci/helm-render/run.sh",\n    ),\n'
    "    # SMA-714. Four lines, like the other self-scheduled gates. Whole-line matched:\n"
    "    # `bash ci/moon-diagnosis/run.sh` is a strict PREFIX of both flagged lines.\n"
    '    "moon-diagnosis-exec": (\n'
    '        "set -euo pipefail",\n'
    '        "bash ci/moon-diagnosis/run.sh --self-test",\n'
    '        "bash ci/moon-diagnosis/run.sh --negative-control",\n'
    '        "bash ci/moon-diagnosis/run.sh",\n'
    "    ),\n}\n",
)

# 4. MOON_DIAGNOSIS_SH_CALL_SITES (obligation 5), after HELM_RENDER_SH_CALL_SITES.
PINS = open(sys.argv[2], encoding="utf-8").read() if len(sys.argv) > 2 else None
assert PINS, "pass the pin-tuple source file as the second argument"
once(
    "    '\"$BASH\" \"$1\" --set ingress.host=console.example.test || raw=$?',\n)\n",
    "    '\"$BASH\" \"$1\" --set ingress.host=console.example.test || raw=$?',\n)\n\n" + PINS,
)

# 5. check_self_invocation: a tenth REQUIRED positional parameter, and its docstring.
once(
    "    next_public_free_sh_text, helm_render_sh_text,\n):",
    "    next_public_free_sh_text, helm_render_sh_text, moon_diagnosis_sh_text,\n):",
)
once(
    "    release-plan, ruff, next-public-free and helm-render gates missing from where they must appear.",
    "    release-plan, ruff, next-public-free, helm-render and moon-diagnosis-exec gates missing from\n"
    "    where they must appear.",
)
once("    Nine haystacks, matched THREE", "    Ten haystacks, matched THREE")
once("    The nine texts are checked", "    The ten texts are checked")
once(
    "`next_public_free_sh_text` and `helm_render_sh_text` are REQUIRED positional parameters",
    "`next_public_free_sh_text`, `helm_render_sh_text` and `moon_diagnosis_sh_text` are REQUIRED\n"
    "    positional parameters",
)

# 6. The tenth haystack.
once(
    "        for site in HELM_RENDER_SH_CALL_SITES\n        if site not in helm_render_lines\n    )\n    return missing\n",
    "        for site in HELM_RENDER_SH_CALL_SITES\n        if site not in helm_render_lines\n    )\n"
    "    # SMA-714 — stripped whole lines, like the helm-render haystack and for the same two reasons.\n"
    "    moon_diagnosis_lines = {line.strip() for line in moon_diagnosis_sh_text.splitlines()}\n"
    "    missing.extend(\n"
    '        f"ci/moon-diagnosis/run.sh: {site}"\n'
    "        for site in MOON_DIAGNOSIS_SH_CALL_SITES\n"
    "        if site not in moon_diagnosis_lines\n"
    "    )\n"
    "    return missing\n",
)

# 7. collect_findings' source keys and call.
once("# The eight shell sources check_self_invocation reads", "# The nine shell sources check_self_invocation reads")
once('    "next_public_free", "helm_render",\n)', '    "next_public_free", "helm_render", "moon_diagnosis",\n)')
once(
    '        sh["workflow_credentials"], sh["release_plan"], sh["ruff"], sh["next_public_free"], sh["helm_render"],\n    )',
    '        sh["workflow_credentials"], sh["release_plan"], sh["ruff"], sh["next_public_free"], sh["helm_render"],\n'
    '        sh["moon_diagnosis"],\n    )',
)

# 8. main(): read the tenth file and hand it over.
once(
    '        helm_render_sh = read_input(\n            root / "ci" / "helm-render" / "run.sh", "ci/helm-render/run.sh"\n        )\n',
    '        helm_render_sh = read_input(\n            root / "ci" / "helm-render" / "run.sh", "ci/helm-render/run.sh"\n        )\n'
    "        moon_diagnosis_sh = read_input(\n"
    '            root / "ci" / "moon-diagnosis" / "run.sh", "ci/moon-diagnosis/run.sh"\n'
    "        )\n",
)
once(
    '                "helm_render": helm_render_sh,\n',
    '                "helm_render": helm_render_sh,\n                "moon_diagnosis": moon_diagnosis_sh,\n',
)

# 9. The call-sites finding's fix text.
once(
    '"    RUFF_SH_CALL_SITES and HELM_RENDER_SH_CALL_SITES in\\n"',
    '"    RUFF_SH_CALL_SITES, HELM_RENDER_SH_CALL_SITES and MOON_DIAGNOSIS_SH_CALL_SITES in\\n"',
)

# 10. self_test(): floor fixture and aligned T.
once(
    '                 # SMA-513 PR 2b — a floor member too, for the same reason.\n                 "helm-render": True},',
    '                 # SMA-513 PR 2b — a floor member too, for the same reason.\n                 "helm-render": True,\n'
    '                 # SMA-714 — a floor member too, for the same reason.\n                 "moon-diagnosis-exec": True},',
)
once(
    '"workflow-credentials", "ruff-ci", "next-public-free", "helm-render"]',
    '"workflow-credentials", "ruff-ci", "next-public-free", "helm-render",\n'
    '                 "moon-diagnosis-exec"]',
)

# 11. self_test(): the wired fixture for the tenth haystack.
once(
    '    wired_helm_render = "".join(\n        f"    {site}\\n" for site in HELM_RENDER_SH_CALL_SITES\n    )\n',
    '    wired_helm_render = "".join(\n        f"    {site}\\n" for site in HELM_RENDER_SH_CALL_SITES\n    )\n'
    "    # SMA-714 — the same shape for ci/moon-diagnosis/run.sh, derived from the registry.\n"
    '    wired_moon_diagnosis = "".join(\n        f"    {site}\\n" for site in MOON_DIAGNOSIS_SH_CALL_SITES\n    )\n',
)

# 12. The REQUIRED-parameter introspection loop.
once(
    '        "helm_render_sh_text",\n    ):',
    '        "helm_render_sh_text", "moon_diagnosis_sh_text",\n    ):',
)

# 13. Every existing check_self_invocation(...) call inside self_test() gets wired_moon_diagnosis as
# its new LAST argument. Calls are found by paren matching, not by regex over the arguments.
start = src.index("def self_test():")
end = src.index("    # _scripts (SMA-553 D10) — a second pure extractor")
body = src[start:end]
out, i, n_calls = [], 0, 0
for m in re.finditer(r"check_self_invocation\(", body):
    if m.start() < i:
        continue
    j, depth = m.end(), 1
    while depth:
        c = body[j]
        depth += (c == "(") - (c == ")")
        j += 1
    inner = body[m.end():j - 1]
    if inner.strip() == "...":
        continue  # the `check_self_invocation(...)` mention inside a docstring
    stripped = inner.rstrip()
    trail = inner[len(stripped):]
    if stripped.endswith(","):
        new_inner = stripped + " wired_moon_diagnosis," + trail
    else:
        new_inner = stripped + ", wired_moon_diagnosis" + trail
    out.append(body[i:m.end()])
    out.append(new_inner + ")")
    i = j
    n_calls += 1
out.append(body[i:])
src = src[:start] + "".join(out) + src[end:]
assert n_calls == 62, f"rewrote {n_calls} self_test calls, expected 62"

# 14. The moon-diagnosis battery, after the helm-render battery.
BATTERY = '''
    # SMA-714 — the helm-render battery above, repeated for the moon-diagnosis haystack: a deletion
    # row per pinned line, a contamination row, a commented-out row and a disabled-guard row.
    for _md_site in MOON_DIAGNOSIS_SH_CALL_SITES:
        _md_broken = "".join(
            line for line in wired_moon_diagnosis.splitlines(keepends=True)
            if line.strip() != _md_site
        )
        if not check_self_invocation(
            wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
            wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render, _md_broken,
        ):
            failures.append(
                f"check_self_invocation: missed {_md_site!r} deleted from ci/moon-diagnosis/run.sh"
            )
    # Contamination: a moon-diagnosis site must not be satisfiable from another haystack.
    if not check_self_invocation(
        wired + wired_moon_diagnosis, scripts, wired_actionlint, wired_release_parity,
        wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free,
        wired_helm_render,
        "".join(line for line in wired_moon_diagnosis.splitlines(keepends=True)
                if line.strip() != MOON_DIAGNOSIS_SH_CALL_SITES[0]),
    ):
        failures.append(
            "check_self_invocation: a moon-diagnosis site was satisfied by run.sh text"
        )
    # Whole-LINE, not substring: a commented-out copy of a pinned line must report missing.
    _md_commented = wired_moon_diagnosis.replace(
        'if [ "$NEGATIVE" = 1 ]; then\\n', '# if [ "$NEGATIVE" = 1 ]; then\\n'
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render, _md_commented,
    ):
        failures.append(
            "check_self_invocation: a COMMENTED-OUT moon-diagnosis line satisfied the pin "
            "(widened to substring matching)"
        )
    # The disabled guard. The negative control's exact row-set compare is what makes it prove
    # anything; comparing the got-rows file with ITSELF leaves every other pinned line
    # byte-identical, so the control would pass whatever rows the old query produced.
    _md_guard_neutered = wired_moon_diagnosis.replace(
        'if ! cmp -s "$TMP/ctl.rows" "$TMP/ctl.want"; then\\n',
        'if ! cmp -s "$TMP/ctl.rows" "$TMP/ctl.rows"; then\\n',
    )
    if not check_self_invocation(
        wired, scripts, wired_actionlint, wired_release_parity, wired_workflow_credentials,
        wired_release_plan, wired_ruff, wired_next_public_free, wired_helm_render,
        _md_guard_neutered,
    ):
        failures.append(
            "check_self_invocation: a NEUTERED moon-diagnosis comparison (always-false guard) "
            "satisfied the pin"
        )
'''
once(
    '            "check_self_invocation: a NEUTERED helm-render comparison (always-false guard) "\n'
    '            "satisfied the pin"\n        )\n',
    '            "check_self_invocation: a NEUTERED helm-render comparison (always-false guard) "\n'
    '            "satisfied the pin"\n        )\n' + BATTERY,
)

open(path, "w", encoding="utf-8").write(src)
print("patched", path)
```

- [ ] **Step 6: Run the self-test BEFORE the edit (baseline) and check each pin is unique**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && python3 ci/affected-graph/ci_targets.py --self-test; echo rc=$? && python3 -c "
import ast, sys
src = open('$TMPDIR/sma714/pins_tuple.py.txt').read()
pins = ast.literal_eval(src.split('= ', 1)[1])
lines = [l.strip() for l in open('ci/moon-diagnosis/run.sh').read().splitlines()]
bad = [p for p in pins if lines.count(p) != 1]
print(len(pins), 'pins;', 'not exactly once:', bad)
assert not bad and len(pins) == 59
assert not [p for p in pins if 'ciReport' in p]
"
```
Expected: `ci-targets self-test OK`, `rc=0`, then `59 pins; not exactly once: []`.

- [ ] **Step 7: Apply the edit script, then run the self-test**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && python3 "$TMPDIR/sma714/sma714_ci_targets_patch.py" ci/affected-graph/ci_targets.py "$TMPDIR/sma714/pins_tuple.py.txt" && python3 ci/affected-graph/ci_targets.py --self-test; echo rc=$? && git diff --stat
```
Expected: `patched ci/affected-graph/ci_targets.py`, `ci-targets self-test OK`, `rc=0`. If the script stops on an assertion, the base moved: read the anchor it names, fix the anchor in the script (not the file by hand), and run again on a clean `ci_targets.py`.

Then run ruff over the file with the repo config:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && uv run --locked --project py ruff check --config py/pyproject.toml ci/affected-graph/ci_targets.py
```
Expected: `All checks passed!`. If the worktree has no `py/.venv` yet, `uv` makes it; that is not host software.

- [ ] **Step 8: Run the real registry check and the affected-graph suite**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && python3 ci/affected-graph/ci_targets.py; echo rc=$? && python3 ci/affected-graph/task_inputs.py; echo rc=$? && /bin/bash ci/affected-graph/run.sh; echo rc=$?
```
Expected: `PASS  ci-targets         -> 34 targets: …`, then `rc=0` three times. `ci/affected-graph/run.sh` needs system `/bin/bash` 3.2 (root `CLAUDE.md`, "This development Mac only"). The files of Task 1 are committed, so `task_inputs.py` sees `ci/moon-diagnosis/**/*` as tracked.

- [ ] **Step 9: Run the task through moon, inside an outer `moon ci` (Review Focus 1)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run repo:moon-diagnosis-exec --force; echo rc=$? && moon ci :moon-diagnosis-exec --base origin/main; echo rc=$?
```
Expected: `rc=0` for both. The second command is the CI shape: the gate runs inside an outer `moon ci`, and its outer-report hash compare must not fire. If it prints `the nested run wrote to the real workspace report`, stop: moon rewrites the outer report during a run, and the hash compare needs a redesign (report this as a blocker with the output).

- [ ] **Step 10: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && git branch --show-current && git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/actionlint/run.sh ci/affected-graph/ci_targets.py && git status --short && git commit -m "feat(ci): register repo:moon-diagnosis-exec in the seven gate places (SMA-714)

Adds the moon task, the T array entry and its CLAUDE.md mirror, the
SELF_SCHEDULED_GATES, SELF_TASK_EXPECTED_GLOBS and REQUIRED_REPO_TASKS
rows, the MOON_DIAGNOSIS_SH_CALL_SITES pin table and its self-test
battery, and the affected-smoke input with its actionlint floor.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
`git status --short` must show no scratchpad file and no other change.

---

### Task 3: The documentation changes

**Files:**
- Modify: `CLAUDE.md` (root) lines 188-190 and 202-204, inside the `moon-diagnosis` block
- Modify: `ci/actionlint/README.md` lines 548-551 (L29) and 561-569 (L32)
- Modify: `ci/CLAUDE.md` (append a section at the end)

**Interfaces:**
- Consumes: the gate of Task 1 (it re-reads the edited block) and the registration of Task 2 (the name `repo:moon-diagnosis-exec`).
- Produces: doc text only. The five `DOC_DIAGNOSIS_REQUIRED_LITERALS` (`operations[]`, `task-execution`, `.moon/cache/states/`, `stderr.log`, `lastRunTime`) stay in the block.

- [ ] **Step 1: Replace the "No gate runs this query" sentence in the root `CLAUDE.md` block**

Replace:
```
  object of the entry whose `meta.type` is `task-execution`. Until SMA-711 this query read the
  two fields from the entry itself, so it printed `null`. No gate runs this query
  (limitation L29 in `ci/actionlint/README.md`, SMA-714).
```
with:
```
  object of the entry whose `meta.type` is `task-execution`. Until SMA-711 this query read the
  two fields from the entry itself, so it printed `null`. `repo:moon-diagnosis-exec` runs this
  query, and checks Steps 2 and 2a, against a real failed report in a fixture workspace (SMA-714).
```
Do not write `**Step 1` or a second `bash` fence into the block: the gate reads that as a second Step 1 query (`multiple-step1`).

- [ ] **Step 2: Replace the Step 2a comparison rule in the same block**

Replace:
```
  **Step 2a — prove the logs belong to this run. Mandatory.** Compare the report action's
  `finishedAt` against `lastRun.json`'s `lastRunTime`. **If they disagree, stop** — the logs are
  from a different run and pairing them with step 1's command yields a confident wrong answer.
```
with:
```
  **Step 2a — prove the logs belong to this run. Mandatory.** Compare the report action's
  `finishedAt` against `lastRun.json`'s `lastRunTime`. `finishedAt` is a UTC time with no zone
  suffix. `lastRunTime` is epoch milliseconds. Moon reads `lastRunTime` first, up to about 16 ms
  before `finishedAt` (measured on macOS and Linux, SMA-714). So a difference from 0 to 1000 ms
  is the same run. **If the difference is negative or more than 1000 ms, stop** — the logs are
  from a different run and pairing them with step 1's command yields a confident wrong answer.
```

- [ ] **Step 3: Run the gate on the edited block and check the markers and literals**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/moon-diagnosis/run.sh; echo rc=$? && /bin/bash ci/moon-diagnosis/run.sh --negative-control; echo rc=$? && for m in moon-diagnosis:begin moon-diagnosis:end ci-targets:begin ci-targets:end; do printf '%s %s\n' "$m" "$(grep -o "$m" CLAUDE.md | wc -l | tr -d ' ')"; done && b="$(sed -n '/moon-diagnosis:begin/,/moon-diagnosis:end/p' CLAUDE.md)" && for lit in 'operations[]' 'task-execution' '.moon/cache/states/' 'stderr.log' 'lastRunTime'; do case "$b" in *"$lit"*) echo "ok $lit" ;; *) echo "MISSING $lit" ;; esac; done
```
Expected: `rc=0` twice; each marker count `1`; five `ok` lines.

- [ ] **Step 4: Narrow L29 in `ci/actionlint/README.md`**

Replace the L29 paragraph (lines 548-551):
```
**L29 (SMA-597).** Check 12 gates the PRESENCE of the procedure's five load-bearing literals,
not its correctness. Editing the `jq` inside CLAUDE.md's block into something subtly wrong stays
green. Closing this needs a gate that EXECUTES the procedure against a deliberately failed task;
that is a follow-up issue, not scope here.
```
with:
```
**L29 (SMA-597, narrowed by SMA-714).** Check 12 gates the PRESENCE of the procedure's five
load-bearing literals, not its correctness. Since SMA-714, `repo:moon-diagnosis-exec`
(`ci/moon-diagnosis/`) runs the Step 1 query and checks Steps 2 and 2a against a real failed
`moon ci` report, so a subtly wrong `jq` in CLAUDE.md's block now reds that gate. These parts stay
open: Step 0 and Step 3 (prose and commands that change state, which no gate runs), the "What
cannot work" paragraph, and the CI note. That gate runs only when `CLAUDE.md`, `.prototools` or
`ci/moon-diagnosis/**` changes.
```

- [ ] **Step 5: Narrow L32 in the same file**

In the L32 paragraph, replace the last sentence:
```
Closing either needs the same procedure-execution gate L29 defers to a follow-up issue, not a bigger token list.
```
(it is wrapped over the last two lines of the paragraph; match the wrapped text exactly) with:
```
`repo:moon-diagnosis-exec` (SMA-714) runs only the root `CLAUDE.md` block. It reads no other
file of the corpus, so this gap stays open for every other file. Closing it needs a check that
reads for meaning, not a bigger token list.
```

- [ ] **Step 6: Append the nested-moon section to `ci/CLAUDE.md`**

Add at the end of the file, after the SMA-708 section, with one blank line before it:

```markdown
## A nested `moon` call that targets a fixture workspace (SMA-714)

- A gate that runs `moon` against a DIFFERENT workspace (a throwaway fixture) runs it with an
  allowlist environment: `env -i HOME="$HOME" PATH="$PATH" TMPDIR=… LANG=… PROTO_HOME=…
  PROTO_REPORTER=text MOON_WORKSPACE_ROOT="<fixture>"`. `ci/moon-diagnosis/run.sh` is the worked
  example.
- Why: moon passes its own `MOON_*` and `PROTO_*` variables to a task. An inherited
  `MOON_WORKSPACE_ROOT` moves the nested run to the real workspace (MEASURED: it failed with
  `app::missing_config` when the variable pointed to another directory). An inherited
  `PROTO_MOON_VERSION` overrides the fixture's `.prototools` pin, because proto reads
  `PROTO_<TOOL>_VERSION` first. One probe also printed `Base revision: N/A` under inherited `CI`
  and `GITHUB_*` variables. A second probe on 2026-09-28 with the same variable names did not
  show it, so that cause is not confirmed. `repo:moon-diagnosis-exec` asserts
  `Base revision: <sha>` on every run.
- Use an allowlist, not a denylist such as `unset MOON_*`. A new variable passes a denylist, and
  `env` output can hold multi-line values.
- This rule does NOT apply to a nested call that must target the REAL workspace, for example the
  nested `moon query` in `repo:affected-smoke`. That call needs the inherited environment.
- A fixture that commits also sets `GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1` on every
  `git` call and local `commit.gpgsign false` and `tag.gpgsign false`. The development Mac signs
  every commit through 1Password, and a locked vault makes an unguarded fixture commit hang or fail.
```

- [ ] **Step 7: Replicate check 12 over the tree**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && git add -A ci/actionlint/README.md ci/CLAUDE.md CLAUDE.md && for f in $(git grep -l ciReport); do case "$f" in ci/actionlint/run.sh|ci/actionlint/README.md|CLAUDE.md) continue ;; esac; grep -q 'moon-diagnosis:superseded\|moon-diagnosis:ok' "$f" || echo "UNMARKED $f"; done; echo done
```
Expected: only `done`. Also confirm that `ci/affected-graph/ci_targets.py` and `moon.yml` are not in `git grep -l ciReport`.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && git branch --show-current && git add CLAUDE.md ci/actionlint/README.md ci/CLAUDE.md && git commit -m "docs(ci): point the moon-diagnosis docs at repo:moon-diagnosis-exec (SMA-714)

The CLAUDE.md block now names the gate that runs its query and states
the Step 2a units and the 0 to 1000 ms tolerance. README L29 and L32
are narrowed to what stays open, and ci/CLAUDE.md records the
allowlist rule for a nested moon call against a fixture workspace.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Verification: the mutation battery, the gate graph, CI shape and Linux

No new file. Record every result (command, predicted rc, actual rc, the arm or row that fired)
for the PR body under a heading "Mutation proof". A mutation is never committed.

**Files:**
- Test only. Temporary edits to `ci/moon-diagnosis/run.sh`, restored with the Edit tool.
- Scratch copies of `CLAUDE.md` in `$TMPDIR/sma714/`.

**Interfaces:**
- Consumes: everything of Tasks 1-3.
- Produces: the "Mutation proof" record for the PR.

- [ ] **Step 1: Doc mutations M-a and M-b (scratch copies of `CLAUDE.md`, no repo edit)**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && mkdir -p "$TMPDIR/sma714" && sed 's/exitCode: \.meta\.exitCode/exitCode: .exitCode/' CLAUDE.md > "$TMPDIR/sma714/ma.md" && sed 's/{status, exitCode: \.meta/{exitCode: .meta/' CLAUDE.md > "$TMPDIR/sma714/mb.md" && ! cmp -s CLAUDE.md "$TMPDIR/sma714/ma.md" && ! cmp -s CLAUDE.md "$TMPDIR/sma714/mb.md" && { /bin/bash ci/moon-diagnosis/run.sh --claude-md "$TMPDIR/sma714/ma.md"; echo "M-a rc=$?"; /bin/bash ci/moon-diagnosis/run.sh --claude-md "$TMPDIR/sma714/mb.md"; echo "M-b rc=$?"; }
```
Expected: M-a `rc=1` with `ROW wrong-exec` for fail3, missing and multi (slow has a null exit code in both forms); M-b `rc=1` with four `ROW wrong-exec` rows.

Also run AC6 on scratch copies (each must give rc 1 with the named row):
````bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && T="$TMPDIR/sma714" && sed '/moon-diagnosis:begin/,/moon-diagnosis:end/d' CLAUDE.md > "$T/noblock.md" && sed 's/^  ```bash$/  ```sh/' CLAUDE.md > "$T/nofence.md" && sed 's#^     \.moon/cache/ciReport\.json$#     .moon/cache/ciReport.json | head#' CLAUDE.md > "$T/shape.md" && sed 's/select(.status=="failed")$/select(.status=="failed"/' CLAUDE.md > "$T/noparse.md" && awk '{print} /^  \*\*Step 2 — why/ && !x {print "  **Step 1 — again.**"; x=1}' CLAUDE.md > "$T/second.md" && for m in noblock nofence shape noparse second; do cmp -s CLAUDE.md "$T/$m.md" && echo "UNCHANGED $m"; /bin/bash ci/moon-diagnosis/run.sh --claude-md "$T/$m.md" > "$T/$m.out" 2>&1; rc=$?; sed -n 's/^ROW /  /p' "$T/$m.out"; echo "$m rc=$rc"; done
````
Expected: `noblock` → `extract no-block`; `nofence` → `extract no-fence`; `shape` → `bad-shape tail-is-not-the-report-path`; `noparse` → `query-error rc=3` (a jq compile error; measured with jq 1.8.2 for this plan); `second` → `extract multiple-step1`. Each `rc=1`, and no `UNCHANGED` line.

- [ ] **Step 2: Script mutations M-c2, M-d, M-d2, M-d3, M-e, M-f, M-g, M-h (one at a time)**

For each row: apply the edit with the Edit tool, run the command, compare with the prediction,
restore with the Edit tool (the exact reverse edit), and confirm `git diff --stat ci/moon-diagnosis/run.sh` prints nothing.

| ID | Edit in `ci/moon-diagnosis/run.sh` | Command | Predicted |
|---|---|---|---|
| M-c2 | in the `nm_rc=0; (cd "$dir" && env -i …` line, replace `--base "$base")` with `--base HEAD~1)` | `/bin/bash ci/moon-diagnosis/run.sh` | rc 2, message `did not use --base` |
| M-d | delete the line `step1_verdict "$TMP/step1.out" > "$TMP/step1.rows" \|\| die_infra "step1_verdict crashed"` | `python3 ci/affected-graph/ci_targets.py` | rc 1, row `ci/moon-diagnosis/run.sh: step1_verdict …` |
| M-d2 | delete the line `step2_verdict "$fx" > "$TMP/step2.rows" \|\| die_infra "step2_verdict crashed"` | `python3 ci/affected-graph/ci_targets.py` | rc 1, row `ci/moon-diagnosis/run.sh: step2_verdict …` |
| M-d3 | replace `if not 0 <= diff_us <= 1000000:` with `if False:` | `/bin/bash ci/moon-diagnosis/run.sh --self-test` | rc 1, FAIL on `u7 Step 2a difference 1001 ms` and `-1 ms` |
| M-e | in `make_fixture`, replace `"    script: 'exit 3'" \` with `"    script: 'exit 0'" \` | `/bin/bash ci/moon-diagnosis/run.sh` | rc 2, `the fixture did not run as designed` |
| M-f | in `make_fixture`, replace `printf 'moon = "%s"\n' "$pin" > "$dir/.prototools"` with `printf 'moon = "2.5.2"\n' > "$dir/.prototools"` | `/bin/bash ci/moon-diagnosis/run.sh` | rc 2, `'moon --version' failed in the fixture` (P5: `missing_tool`) |
| M-g | delete the line `if ! cmp -s "$TMP/ctl.rows" "$TMP/ctl.want"; then` and its body up to the matching `fi` | `python3 ci/affected-graph/ci_targets.py` | rc 1, row `ci/moon-diagnosis/run.sh: if ! cmp -s …` |
| M-h | in `STEP1_VERDICT_JQ`, replace `if length == 0 then "empty-output"` with `if length == 0 then "empty-output" +` | `/bin/bash ci/moon-diagnosis/run.sh` | rc 1, `ROW verdict-error step1 rc=3` (measured with jq 1.8.2), never rc 0 |

`python3 ci/affected-graph/ci_targets.py` calls `moon query`; run it with the PATH prefix.

- [ ] **Step 3: M-c, the plain environment inside `moon run`, in a SCRATCH CLONE**

Never run this in the main checkout or in the worktree. Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && C="$TMPDIR/sma714/clone" && rm -rf "$C" && git clone -q /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate "$C" && cd "$C" && git checkout -q feature/sma-714-moon-diagnosis-exec-gate && git log --oneline -1
```
In `$C/ci/moon-diagnosis/run.sh` (the clone, with the Edit tool), in the `nm_rc=0; (cd "$dir" && env -i …` line, delete the text from `env -i ` up to and including `MOON_WORKSPACE_ROOT="$dir" ` so that the line runs `moon ci probe:fail3 …` with the inherited environment. Then run:
```bash
cd "$TMPDIR/sma714/clone" && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run repo:moon-diagnosis-exec --force; echo "M-c rc=$?"; cat .moon/cache/states/repo/moon-diagnosis-exec/stderr.log | tail -n 5
```
Predicted: the task fails, and its stderr holds `infrastructure error (rc=2)`. Record which arm fired: `the nested run wrote to the real workspace report`, or `no valid fixture report`. Then `rm -rf "$TMPDIR/sma714/clone"`.

- [ ] **Step 4: T5, the gate graph**

Run the gates that the registration touches:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && /bin/bash ci/affected-graph/run.sh; echo "affected-smoke rc=$?"; python3 ci/affected-graph/task_inputs.py; echo "input-liveness rc=$?"; /opt/homebrew/bin/bash ci/actionlint/run.sh > "$TMPDIR/sma714-al.out" 2>&1; echo "actionlint rc=$?"; tail -n 15 "$TMPDIR/sma714-al.out"
```
Expected: `rc=0` for all three. For `repo:actionlint`, read the preflight line first. If it says the pipe holds 512 bytes (rc 2, `small`), run the gate in a Linux container instead, per the memory "Small pipe: run gates in a Linux container" (SMA-685): mount a `git archive HEAD` export of the worktree read-only, install `actionlint` 1.7.12 and the other tools through proto in the container, and run `bash ci/actionlint/run.sh`. Check 12 (markers) and check 13 (readers) must show no FAIL row. CI is the final verdict.

- [ ] **Step 5: T6 and T7, the CI environment and Linux**

T6 (macOS): run the real mode with a CI-shaped OUTER environment:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && CI=true GITHUB_ACTIONS=true GITHUB_SHA=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef GITHUB_BASE_REF=main GITHUB_HEAD_REF=x GITHUB_EVENT_NAME=pull_request GITHUB_REF=refs/pull/1/merge PROTO_MOON_VERSION=2.4.6 MOON_WORKSPACE_ROOT=/nonexistent /bin/bash ci/moon-diagnosis/run.sh; echo "T6 rc=$?"
```
Expected: `T6 rc=0` (the gate asserts `Base revision: <sha>` itself).

T7 (Linux): save this script as `$TMPDIR/sma714/linux-gate.sh`:
```bash
#!/bin/bash
set -u
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null && apt-get install -y -qq curl git jq python3 xz-utils unzip ca-certificates >/dev/null 2>&1 || { echo APT_FAIL; exit 9; }
curl -fsSL https://moonrepo.dev/install/proto.sh -o /tmp/proto.sh && bash /tmp/proto.sh 0.61.1 --yes --no-profile >/tmp/pi.log 2>&1 || { echo PROTO_FAIL; exit 9; }
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
proto install moon 2.5.3 >/tmp/mi.log 2>&1 || { echo MOON_FAIL; exit 9; }
mkdir -p /g && tar -xf /p/tree.tar -C /g && cd /g
for m in --self-test "" --negative-control; do
  s=$(date +%s%N); bash ci/moon-diagnosis/run.sh $m > /tmp/o 2>&1; rc=$?; e=$(date +%s%N)
  echo "mode=${m:-real} rc=$rc ms=$(( (e-s)/1000000 ))"; tail -n 2 /tmp/o
done
```
Then run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && git archive -o "$TMPDIR/sma714/tree.tar" HEAD && docker run --rm -v "$TMPDIR/sma714":/p:ro ubuntu:24.04 bash /p/linux-gate.sh
```
Expected: three `rc=0` lines (self-test about 0.1 s, the two nested modes about 7 s each).

- [ ] **Step 6: Review Focus 2, a host config that forces signing**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && H="$TMPDIR/sma714/home" && mkdir -p "$H" && printf '[commit]\n\tgpgsign = true\n[gpg]\n\tformat = ssh\n[gpg "ssh"]\n\tprogram = /usr/bin/false\n' > "$H/.gitconfig" && HOME="$H" PROTO_HOME="$HOME/.proto" /bin/bash ci/moon-diagnosis/run.sh; echo "signing rc=$?"
```
Expected: `signing rc=0`. The fixture ignores the global config. If moon itself fails because of the new `HOME` (not a git signing error), record the error text and run the same check with `GIT_CONFIG_GLOBAL="$H/.gitconfig"` and the real `HOME` instead.

- [ ] **Step 7: Review Focus 3, a repository path with a space**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-714-moon-diagnosis-exec-gate && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && D="$TMPDIR/sma714/with space" && rm -rf "$D" && mkdir -p "$D" && git archive HEAD | tar -xf - -C "$D" && for m in --self-test "" --negative-control; do /bin/bash "$D/ci/moon-diagnosis/run.sh" $m > "$TMPDIR/sma714/sp.out" 2>&1; echo "mode=${m:-real} rc=$?"; done; rm -rf "$D"
```
Expected: three `rc=0` lines.

- [ ] **Step 8: Re-run the whole battery after any fix**

If any step of this task needed a fix in Tasks 1-3, commit the fix (a new commit, never an amend), then run Task 4 Steps 1 to 7 again in full, not only the failed row (memory "Re-run a mutation battery whole"). Confirm at the end: `git status --short` is clean and `git diff origin/main --stat` lists only the nine files of the file-structure table.

---

## Self-review record

- Spec coverage: §2 files → Tasks 1-3; §3 units 1-7 → Task 1 (script) with self-test cases per spec §4; §4 modes and rc map → Task 1; §5 task → Task 2 Step 1; §6 obligations 1-7 → Task 2 Steps 2-7; §7 docs → Task 3; AC1 → Task 4 Step 1 (M-a) and the negative control; AC2 → Task 3 Steps 4-5; AC3 → Task 1 Step 5, Task 4 Steps 5-6, CI; AC4 → Task 1 Step 5; AC5 → the `env -i` lines and hash compare (Task 1), Task 4 Step 3; AC6 → Task 4 Step 1; AC7 → self-test u7 rows and Task 3 Step 2; AC8 → Task 2 Steps 8-9 and Task 4 Step 4; AC9 → self-test `u6 verdict tool fails` and M-h. T1-T7 → Task 1 Steps 3-4, Task 1 Step 5, Task 4 Steps 1-5.
- Deviations from the spec, each deliberate: (1) the `Base revision: <sha>` assertion in unit 4 (P6); (2) `step2_verdict` also emits `step2a-no-action` and `step2a-no-lastrun` rows, so a missing timestamp is a red and not a crash; (3) `run_step1_query` emits `query-error rc=<n>` as a row, so the real path reports it through the same `ROW` channel; (4) the self-test covers 37 cases plus the count row, more than the spec minimum.
- Names used across tasks: `MOON_DIAGNOSIS_SH_CALL_SITES`, `moon_diagnosis_sh_text`, `wired_moon_diagnosis`, `_CALL_SITE_SOURCE_KEYS` key `moon_diagnosis`, task key `moon-diagnosis-exec`, `SELF_TEST_CASES=37`.
