# SMA-709 COLLECTION_ROWS Floors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Raise the two twin `COLLECTION_ROWS` floors from 14 to 37 (the row count minus one), so
that a removal of two or more rows reds both the in-process self-test and `repo:actionlint`
check 11.

**Architecture:** Two integer literals change in the same commit: one in
`ci/release-plan/release_plan.py` `self_test()`, one in `ci/actionlint/run.sh`
`release_plan_self_test()`. Both failure messages and both comments get the re-baseline rule.
Two prose sites and one stale comment stop stating a row count. A scratch proof harness (not
committed) runs the mutation proofs T1 to T4 and T7 against copies, never against tracked files.

**Tech Stack:** Python 3.12 (stdlib only, run through `uv`), bash, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-28-sma-709-collection-rows-floor-design.md`

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-709-collection-rows-floor`,
  branch `feature/sma-709-collection-rows-floor`. Begin every Bash command with
  `cd <worktree> &&`. Before each commit, `git branch --show-current` must print
  `feature/sma-709-collection-rows-floor`.
- Command prefix: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text`.
- Floor value N = `--collection-count` − 1. On the base `906de46c` the count is 38, so N = 37.
  Approved by Sven on 2026-09-28 (Q2). If a rebase changes the count, set both floors to the new
  count minus one.
- The gate-side floor keeps `infra` (rc 2). Approved by Sven on 2026-09-28 (Q1).
- Both floor values change in the SAME commit (AC1).
- Keep the substring `COLLECTION_ROWS has only {n} row(s); the floor is 37` (T3 asserts it) and
  the substring `reports $c collection rows, expected at least 37` (T7 asserts it).
- Mutations run on COPIES in a scratch directory only. Never edit a tracked file to prove a
  mutation. Remove rows BY LABEL (an `ast` parse), never by line number.
- No SPDX header or other text changes. No new mention of moon's CI report file in any doc
  (actionlint check 12 needs a `moon-diagnosis` marker for it).
- Conventional commits with the `ci` scope. End every commit message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never use `--amend`, `git reset`,
  `--no-verify` or `--no-gpg-sign`.
- Do not install host software. Do not leave background jobs running.

## Review Focus

1. The row count changes after a rebase (another ticket adds rows). The harness reads the count
   from the branch file and derives every expected number from it, so it does not assume 38.
   Task 1 Step 1 pins this.
2. A mutant that does not parse (rc 1) can hide as "not rc 3". The harness asserts that each
   mutant's `--collection-count` prints the expected count before it reads `--self-test`.
3. A renamed `release_plan_self_test()` makes the `sed` extraction empty, and T7 then passes
   vacuously. The harness exits 2 on an empty extraction and on an extraction that does not
   contain `--collection-count`.
4. Only one of the two floors moves. The harness proves both sides (T3 for the in-process floor,
   T7 for the gate floor) with a red-first control against `origin/main` for each.
5. A quote or a backtick in the new bash `infra "…"` message breaks the line or runs a command
   substitution. The new message contains neither; T7 asserts the exact stderr substring.

---

### Task 1: Raise both COLLECTION_ROWS floors to 37

**Files:**
- Create (scratch, NOT committed): `<SCRATCH>/sma-709/drop_rows.py`, `<SCRATCH>/sma-709/prove-floors.sh`
  (`<SCRATCH>` is your session scratchpad directory from the system prompt)
- Modify: `ci/release-plan/release_plan.py:1778-1786` (`self_test()` comment, floor, message)
- Modify: `ci/actionlint/run.sh:4644-4650` (check 11 twin comment, floor, message)

**Interfaces:**
- Consumes: `release_plan.py --collection-count` (prints `len(COLLECTION_ROWS)`),
  `release_plan.py --self-test` (rc 0 or 3), `release_plan_self_test()` in `ci/actionlint/run.sh`
  (calls `infra`, `fail`, `release_plan_sh`, reads `SELF_TESTS_RAN`).
- Produces: floors at 37 in both files; the harness `prove-floors.sh`, which Task 3 re-runs.

- [ ] **Step 1: Write the row-removal helper**

Write `<SCRATCH>/sma-709/drop_rows.py`:

```python
# SPDX-License-Identifier: Apache-2.0
"""drop_rows.py SRC DST LABEL... -- copy SRC to DST without the named COLLECTION_ROWS rows."""
import ast
import sys

src, dst, *labels = sys.argv[1:]
if not labels:
    sys.exit("drop_rows: no label given")
text = open(src, encoding="utf-8").read()
tree = ast.parse(text)
lines = text.splitlines(keepends=True)
rows = None
for node in tree.body:
    if isinstance(node, ast.AnnAssign) and getattr(node.target, "id", None) == "COLLECTION_ROWS":
        rows = node.value
if not isinstance(rows, ast.Tuple):
    sys.exit("drop_rows: COLLECTION_ROWS tuple not found")
drop: set[int] = set()
for label in labels:
    hits = [
        e for e in rows.elts
        if isinstance(e, ast.Tuple) and isinstance(e.elts[0], ast.Constant) and e.elts[0].value == label
    ]
    if len(hits) != 1:
        sys.exit(f"drop_rows: label {label!r} matched {len(hits)} rows")
    e = hits[0]
    first, last = lines[e.lineno - 1], lines[e.end_lineno - 1]
    if not first.lstrip().startswith("(") or not last.rstrip().endswith("),"):
        sys.exit(f"drop_rows: row {label!r} shares a line with other code")
    drop.update(range(e.lineno - 1, e.end_lineno))
out = "".join(line for i, line in enumerate(lines) if i not in drop)
ast.parse(out)  # the copy must still parse
open(dst, "w", encoding="utf-8").write(out)
```

- [ ] **Step 2: Write the proof harness**

Write `<SCRATCH>/sma-709/prove-floors.sh`:

```bash
#!/bin/bash
# SMA-709 proof harness: T1-T4 (in-process floor) and T7 (gate floor). Copies only.
set -u
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-709-collection-rows-floor
HERE="$(cd "$(dirname "$0")" && pwd)"
S="$(mktemp -d "${TMPDIR:-/tmp}/sma709.XXXXXX")"
bad=0
ok() { echo "PASS $*"; }
ko() { echo "FAIL $*"; bad=1; }
die() { echo "INFRA $*"; exit 2; }
py() { (cd "$WT" && uv run --locked --project ci/release-plan --python '>=3.12' python3 "$@"); }

git -C "$WT" show origin/main:ci/release-plan/release_plan.py > "$S/main.py" || die "git show main.py"
git -C "$WT" show origin/main:ci/actionlint/run.sh > "$S/main-run.sh" || die "git show main run.sh"
cp "$WT/ci/release-plan/release_plan.py" "$S/branch.py"

L708="SMA-708 a committed fixture repository starts no background git maintenance"
L688="SMA-688 sed parity: a quoted chain key fails --assert"
mk() { # SRC DST LABEL...
  py "$HERE/drop_rows.py" "$@" || die "drop_rows failed for $2"
  cmp -s "$1" "$2" && die "mutation made no change: $2"
  return 0
}
mk "$S/branch.py" "$S/b1.py" "$L708"
mk "$S/branch.py" "$S/b2.py" "$L708" "$L688"
mk "$S/main.py" "$S/m2.py" "$L708" "$L688"

# T1: the unmodified branch file.
C="$(py ci/release-plan/release_plan.py --collection-count)" || die "--collection-count failed"
case "$C" in ''|*[!0-9]*) die "--collection-count printed '$C'" ;; esac
F=$((C - 1))
echo "INFO count=$C expected floor=$F"
py ci/release-plan/release_plan.py --self-test 2>"$S/t1.err" && ok "T1 branch --self-test rc 0" \
  || { ko "T1 branch --self-test rc $?"; cat "$S/t1.err"; }

# T2: one row removed -> count C-1, rc 0.
got="$(py "$S/b1.py" --collection-count)"
[ "$got" = "$((C - 1))" ] && ok "T2 one-row copy parses, count $got" || ko "T2 count '$got', want $((C - 1))"
py "$S/b1.py" --self-test 2>"$S/t2.err"; rc=$?
[ $rc -eq 0 ] && ok "T2 one-row copy --self-test rc 0" || { ko "T2 rc $rc, want 0"; cat "$S/t2.err"; }

# T3: two rows removed -> count C-2, rc 3 WITH the floor text.
got="$(py "$S/b2.py" --collection-count)"
[ "$got" = "$((C - 2))" ] && ok "T3 two-row copy parses, count $got" || ko "T3 count '$got', want $((C - 2))"
py "$S/b2.py" --self-test 2>"$S/t3.err"; rc=$?
want="COLLECTION_ROWS has only $((C - 2)) row(s); the floor is $F"
[ $rc -eq 3 ] && ok "T3 two-row copy --self-test rc 3" || ko "T3 rc $rc, want 3"
grep -qF "$want" "$S/t3.err" && ok "T3 stderr names the floor" || ko "T3 stderr lacks: $want"

# T4: red-first control, origin/main's file with the same two rows removed -> rc 0.
py "$S/m2.py" --self-test 2>"$S/t4.err"; rc=$?
[ $rc -eq 0 ] && ok "T4 origin/main two-row copy rc 0 (defect reproduced)" || ko "T4 rc $rc, want 0"

# T7: the real release_plan_self_test() from a run.sh, against a scratch root.
root() { # NAME MUTANT
  mkdir -p "$S/$1/ci/release-plan"
  cp "$WT/ci/release-plan/pyproject.toml" "$WT/ci/release-plan/uv.lock" "$S/$1/ci/release-plan/"
  cp "$2" "$S/$1/ci/release-plan/release_plan.py"
}
root r1 "$S/b1.py"
root r2 "$S/b2.py"
gate() { # ROOT RUNSH ERRFILE ; prints nothing, returns the function's rc
  local fn
  fn="$(sed -n '/^release_plan_self_test() {/,/^}/p' "$2")"
  [ -n "$fn" ] || die "empty release_plan_self_test extraction from $2"
  case "$fn" in *--collection-count*) ;; *) die "extraction from $2 has no --collection-count" ;; esac
  (cd "$S/$1" && FN="$fn" /bin/bash -c 'infra(){ echo "INFRA: $*" >&2; exit 2; }
    fail(){ echo "FAIL: $*" >&2; }
    release_plan_sh(){ :; }
    SELF_TESTS_RAN=0
    eval "$FN"
    release_plan_self_test' 2>"$3")
}
gate r2 "$WT/ci/actionlint/run.sh" "$S/t7a.err"; rc=$?
want="reports $((C - 2)) collection rows, expected at least $F"
[ $rc -eq 2 ] && ok "T7 branch gate, two-row copy rc 2" || ko "T7 branch gate two-row rc $rc, want 2"
grep -qF "$want" "$S/t7a.err" && ok "T7 stderr names the floor" || { ko "T7 stderr lacks: $want"; cat "$S/t7a.err"; }
gate r1 "$WT/ci/actionlint/run.sh" "$S/t7b.err"; rc=$?
[ $rc -eq 0 ] && ok "T7 branch gate, one-row copy rc 0" || { ko "T7 one-row rc $rc, want 0"; cat "$S/t7b.err"; }
gate r2 "$S/main-run.sh" "$S/t7c.err"; rc=$?
[ $rc -eq 0 ] && ok "T7 origin/main gate, two-row copy rc 0 (red-first)" || { ko "T7 main rc $rc, want 0"; cat "$S/t7c.err"; }

[ $bad -eq 0 ] && echo "ALL PASS" || echo "SOME FAIL"
exit $bad
```

- [ ] **Step 3: Run the harness before the change (red-first)**

Run: `cd <worktree> && /bin/bash <SCRATCH>/sma-709/prove-floors.sh`
Expected: `INFO count=38 expected floor=37` and exactly four FAIL lines:

```text
FAIL T3 rc 0, want 3
FAIL T3 stderr lacks: COLLECTION_ROWS has only 36 row(s); the floor is 37
FAIL T7 branch gate two-row rc 0, want 2
FAIL T7 stderr lacks: reports 36 collection rows, expected at least 37
```

Every T1, T2, T4 and other T7 line is PASS, the last line is `SOME FAIL`, exit 1. (`uv` lines
such as `Creating virtual environment` can appear after a FAIL line; they are the captured stderr.)
An `INFRA` line means the harness is broken; fix it before you continue. The plan author ran
this harness on 2026-09-28 against the unchanged branch and got exactly this result.

- [ ] **Step 4: Raise the in-process floor**

In `ci/release-plan/release_plan.py`, replace:

```python
    # the gate as infra. Twinned by check 11's --collection-count floor in ci/actionlint/run.sh,
    # in a separately scheduled file, so one edit cannot remove both.
    if len(COLLECTION_ROWS) < 14:
        print(f"FAIL COLLECTION_ROWS has only {len(COLLECTION_ROWS)} row(s); the floor is 14 — "
              "something emptied or gutted the collection-layer table", file=sys.stderr)
        rc = 3
```

with:

```python
    # the gate as infra. Twinned by check 11's --collection-count floor in ci/actionlint/run.sh,
    # in a separately scheduled file, so one edit cannot remove both.
    # The floor tracks the table (SMA-709). When the row count changes, in either direction, set
    # the floor to one below `--collection-count`. Change the twin in ci/actionlint/run.sh
    # check 11 in the same commit.
    if len(COLLECTION_ROWS) < 37:
        print(f"FAIL COLLECTION_ROWS has only {len(COLLECTION_ROWS)} row(s); the floor is 37 — "
              "something emptied or gutted the collection-layer table; if the removal is "
              "intended, lower both floors in the same commit (SMA-709)", file=sys.stderr)
        rc = 3
```

- [ ] **Step 5: Raise the gate-side floor**

In `ci/actionlint/run.sh`, replace:

```bash
  # The COLLECTION_ROWS twin. Separate flag, not a widened --fixture-count: that flag's consumer
  # above validates a single integer, and one number cannot floor two tables.
```

with:

```bash
  # The COLLECTION_ROWS twin. Separate flag, not a widened --fixture-count: that flag's consumer
  # above validates a single integer, and one number cannot floor two tables.
  # The floor tracks the table (SMA-709): one below the current `--collection-count`. When the
  # row count changes, in either direction, re-set this floor and its twin in
  # ci/release-plan/release_plan.py self_test() in the same commit.
```

and replace:

```bash
  [ "$c" -ge 14 ] || infra "check 11: release_plan.py reports $c collection rows, expected at least 14"
```

with:

```bash
  [ "$c" -ge 37 ] || infra "check 11: release_plan.py reports $c collection rows, expected at least 37; if the removal is intended, lower both floors in the same commit (SMA-709)"
```

If the count printed in Step 3 was not 38, use that count minus one in place of 37 in Steps 4
and 5.

- [ ] **Step 6: Run the harness after the change (green)**

Run: `cd <worktree> && /bin/bash <SCRATCH>/sma-709/prove-floors.sh`
Expected: every line PASS, last line `ALL PASS`, exit 0. The T3 and T7 lines that failed in
Step 3 now pass; T4 and the T7 red-first line still pass against `origin/main`.

- [ ] **Step 7: Check the diff**

Run: `cd <worktree> && git diff --stat && git diff`
Expected: only `ci/release-plan/release_plan.py` and `ci/actionlint/run.sh` change; both show
`37` in place of `14`; no scratch file is inside the worktree (`git status --short` lists only
the two files).

- [ ] **Step 8: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-709-collection-rows-floor
git add ci/release-plan/release_plan.py ci/actionlint/run.sh
git commit -m "fix(ci): raise the COLLECTION_ROWS floors to track the table (SMA-709)

Both twin floors move from 14 to 37, one below the 38-row table. A
removal of two or more rows now reds the in-process self-test (rc 3)
and repo:actionlint check 11 (infra, rc 2). Both messages tell the
author to lower both floors in the same commit when a removal is
intended.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Do not put a `#NNN` or a `token: value` line in the body (commitlint `footer-leading-blank`).

### Task 2: Stop stating a row count in the prose and the stale check 10 citation

**Files:**
- Modify: `ci/release-plan/README.md:153-155` (row 8 prose)
- Modify: `ci/release-plan/run.sh:448-452` (row 8 comment)
- Modify: `ci/actionlint/run.sh:4638-4641` (FIXTURES floor comment in check 11)

**Interfaces:**
- Consumes: nothing from Task 1 except that the floors exist.
- Produces: text only. No behavior changes.

- [ ] **Step 1: Confirm the old text is present (red-first)**

Run:
`cd <worktree> && grep -n -e "short two or more" -e "delete two rows" -e "150 against 155" ci/release-plan/README.md ci/release-plan/run.sh ci/actionlint/run.sh`
Expected: three hits (README.md:154, ci/release-plan/run.sh:449, ci/actionlint/run.sh:4640).

- [ ] **Step 2: Reword the README row 8 prose**

In `ci/release-plan/README.md`, replace:

```markdown
  because rc 3 alone is not sufficient: `self_test()`'s own arity floor also returns 3 if
  `COLLECTION_ROWS` is short two or more rows, so an rc-only check would go green whether the
```

with:

```markdown
  because rc 3 alone is not sufficient: `self_test()`'s own arity floor also returns 3 when
  `COLLECTION_ROWS` falls below its floor, so an rc-only check would go green whether the
```

- [ ] **Step 3: Reword the run.sh row 8 comment**

In `ci/release-plan/run.sh`, replace:

```bash
  # It asserts on STDERR as well as rc. rc 3 alone is satisfiable by the arity floor in
  # self_test(): delete two rows from COLLECTION_ROWS and the floor fires, self_test() returns 3
  # FOR THE FLOOR, and an rc-only assertion goes green while the neutered check went undetected —
  # the two controls covering for each other's absence. Rows 3/4 grep for a specific verdict line
  # for the same reason.
```

with:

```bash
  # It asserts on STDERR as well as rc. rc 3 alone is satisfiable by the arity floor in
  # self_test(): delete rows from COLLECTION_ROWS until it falls below its floor, and the floor
  # fires, self_test() returns 3 FOR THE FLOOR, and an rc-only assertion goes green while the
  # neutered check went undetected — the two controls covering for each other's absence. Rows 3/4
  # grep for a specific verdict line for the same reason.
```

Before the edit, read lines 446-453 and confirm the five old lines match exactly (the spec
quotes line 449; the last two lines are from the current file).

- [ ] **Step 4: Remove the stale check 10 citation**

In `ci/actionlint/run.sh`, replace:

```bash
  # Floor, not a count: it exists to catch an EMPTIED table, and one row of headroom keeps a
  # legitimate row removal from aborting the gate as infra. Check 10's own floor is equally
  # loose (150 against 155 actual — that citation read 20 against 84 until the SMA-658 PR 2
  # review; the table has grown with every V8/V9 round since).
```

with:

```bash
  # Floor, not a count: it exists to catch an EMPTIED table, and one row of headroom keeps a
  # legitimate row removal from aborting the gate as infra. Check 10's own floor follows the same
  # one-row headroom rule.
```

- [ ] **Step 5: Verify the text and the release-plan gate**

Run:
`cd <worktree> && grep -n -e "short two or more" -e "delete two rows" -e "150 against 155" -e "20 against 84" ci/release-plan/README.md ci/release-plan/run.sh ci/actionlint/run.sh; echo "grep rc=$?"`
Expected: no hit, `grep rc=1` (AC8, AC9).

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && /bin/bash ci/release-plan/run.sh --self-test; echo "rc=$?"`
Expected: `rc=0`.

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && /bin/bash ci/release-plan/run.sh --negative-control; echo "rc=$?"`
Expected: `rc=0` (T5). `ci/release-plan/run.sh` uses no `mapfile` and no `declare -A`, so
system bash 3.2 is correct for it.

Run: `cd <worktree> && /bin/bash -n ci/actionlint/run.sh && /bin/bash -n ci/release-plan/run.sh && echo syntax-ok`
Expected: `syntax-ok`.

- [ ] **Step 6: Commit**

```bash
cd <worktree> && git branch --show-current   # must print feature/sma-709-collection-rows-floor
git add ci/release-plan/README.md ci/release-plan/run.sh ci/actionlint/run.sh
git commit -m "docs(ci): stop stating a row count for the COLLECTION_ROWS floor (SMA-709)

The row 8 prose in the release-plan README and run.sh now says the
floor fires when the table falls below its floor, not when it is short
two rows. The check 11 FIXTURES comment no longer cites a check 10 row
count.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 3: Verify the gate graph and the row count before the push

**Files:**
- No planned file changes. A fix found here gets its own commit with the `ci` scope.

**Interfaces:**
- Consumes: the harness `<SCRATCH>/sma-709/prove-floors.sh` from Task 1; both commits.
- Produces: a verification record for the PR body (rc of each gate, the bash that ran it, the
  pipe state, and any AC whose verdict is CI only).

- [ ] **Step 1: Re-check the row count against the current origin/main**

Run: `cd <worktree> && git fetch -q origin +refs/heads/main:refs/remotes/origin/main && git log --oneline HEAD..origin/main -- ci/release-plan ci/actionlint/run.sh`
Expected: no output. If there is output, rebase onto `origin/main`
(`git rebase origin/main`), then re-run the harness. If `INFO count=` is not 38, set both floors
to the new count minus one in one new `fix(ci)` commit and re-run the harness until `ALL PASS`.

Run: `cd <worktree> && /bin/bash <SCRATCH>/sma-709/prove-floors.sh`
Expected: `ALL PASS` (AC1, AC4, AC6, AC7).

- [ ] **Step 2: Run the full ci-targets graph**

Run the command between the `ci-targets` markers in the root `CLAUDE.md`, with the command prefix
and a Bash tool timeout of 600000:

```bash
cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :http-extractor-envelope :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci :next-public-free :helm-render :test-e2e --base origin/main --include-relations
```

Expected: green, except gates that fail only because of the bash split on this Mac. For each
failed task, follow root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" Step 0 to
Step 2a before any re-run. An empty stdout with a `mapfile` or `declare -A` stderr line is a
bash-version artifact, not a finding.

- [ ] **Step 3: Re-run the bash-sensitive gates directly (T8)**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && /opt/homebrew/bin/bash ci/ruff/run.sh --self-test && /opt/homebrew/bin/bash ci/ruff/run.sh --negative-control && /opt/homebrew/bin/bash ci/ruff/run.sh; echo "rc=$?"`
Expected: `rc=0` (`repo:ruff-ci` needs bash 4+).

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && /bin/bash ci/affected-graph/run.sh --negative-control && /bin/bash ci/affected-graph/run.sh; echo "rc=$?"`
Expected: `rc=0` (`repo:affected-smoke` needs system bash 3.2).

If a bash-5 run hangs (0% CPU, no output), treat it as the SMA-612 small-pipe state, not as a
gate result.

- [ ] **Step 4: Run repo:actionlint (T6)**

Run: `cd <worktree> && export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text && /opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"`
Read the pipe preflight line first. Expected on a healthy pipe (`pipe capacity 65536 bytes`):
`rc=0` with no FAIL row. If the gate exits 2 with a `small` pipe message, try a Linux container
(`docker run --rm -v <worktree>:/w -w /w python:3.12-bookworm bash ci/actionlint/run.sh`, one
docker command per Bash call). That image has no actionlint, shellcheck or uv, so the run can
fail for a missing tool; that is not a verdict. Then record "repo:actionlint: no local verdict
(small pipe, SMA-612); the CI result is the verdict for AC5" for the PR body.

- [ ] **Step 5: Confirm the branch state**

Run: `cd <worktree> && git status --short && git log --oneline origin/main..HEAD`
Expected: a clean tree and four commits: `docs(ci): spec for SMA-709`,
`docs(ci): plan for SMA-709`, the Task 1 `fix(ci)` commit and the Task 2 `docs(ci)` commit
(plus any re-baseline commit from Step 1). No scratch file is tracked, and
`ci/release-plan/.venv` is not in `git status` (it is ignored).
