# SMA-612 actionlint pipe-capacity preflight — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ci/actionlint/run.sh` exit rc 2 in seconds, with a clear message, on a host where a new pipe holds fewer than 8192 bytes, instead of hanging silently.

**Architecture:** A pure verdict function (`pipe_capacity_verdict`) with its own fixture table (the sixteenth self-test), and a preflight function (`pipe_capacity_preflight`) that runs a Python pipe probe and routes the verdict. The preflight is called from one column-0 line directly after the argument `case`, before `run_self_tests`, in full-gate mode only. `ci/affected-graph/ci_targets.py` pins the call line and the preflight's routing lines.

**Tech Stack:** bash (must run under `/bin/bash` 3.2.57 AND bash 5.x), Python 3 via `uv run --locked --project py`, Python 3 for `ci_targets.py`.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md` (approved 2026-09-19). Read it first; section numbers below (§N, DN) refer to it.

## Global Constraints

- Floor: `PIPE_CAPACITY_FLOOR=8192` (D3).
- Exit codes: every probe-related failure goes through the existing `infra` function (rc 2). Never `fail` (rc 1), never rc 0 (D4, D5).
- The preflight runs ONLY when `SELF_TEST_ONLY=0`, and BEFORE `run_self_tests` (D1, D2).
- No bypass environment variable (D6).
- bash 3.2 compatible: no `mapfile`, no `declare -A`, no `[[ =~ ]]`, no here-strings (`<<<`) in new code.
- Check 13 (`ci/actionlint/run.sh`): never pipe into `grep -q`, `grep -m`, `head`, or an `awk … exit`. `| tail -n1` is allowed.
- Check 12: new text must NOT contain the literal file name of Moon's CI report (the JSON file under `.moon/cache/` that CLAUDE.md's diagnosis procedure names). Do not write it in code, comments, docs or commit messages.
- Every source file keeps its SPDX header. No new files are needed in `ci/`.
- Do not edit the `<!-- ci-targets:… -->` or `<!-- moon-diagnosis:… -->` marker blocks in CLAUDE.md, and do not quote those markers anywhere.
- Commits: conventional, scope `ci`, subject ends `(SMA-612)`, body ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Never `--no-verify`. Never `git commit --amend`. Never `git stash`. Add, do not amend.
- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-612-shellcheck-hang`, branch `feature/sma-612-actionlint-shellcheck-hang`. Check `git branch --show-current` before the first commit.
- PATH prefix for every shell: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- If the sandbox refuses a direct `bash …` command, write the command into a script file in the session scratchpad and run `/bin/bash <that file>`.
- Messages and comments in Simplified Technical English: short sentences, active voice, no idiom.

## Known local baseline (so a subagent does not misread it)

- `/bin/bash ci/actionlint/run.sh --self-test` on this host exits rc 1 with exactly TWO false `cargo-lock-step` failure rows (CLAUDE.md, SMA-647). Those two rows are expected. Any OTHER failure row is real.
- `/opt/homebrew/bin/bash ci/actionlint/run.sh --self-test` HANGS on this host (spec M8). Never run it without a time limit. This plan does not fix `--self-test` under that bash (spec R2).
- On this host a new pipe holds 512 bytes (spec M4). So after Task 2 the full gate must exit rc 2 here, under both bashes.
- `python3 ci/affected-graph/ci_targets.py --self-test` and `python3 ci/affected-graph/ci_targets.py` both exit 0 on the base commit (measured 2026-09-19).

---

### Task 1: The verdict function and the sixteenth fixture table

**Files:**
- Modify: `ci/actionlint/run.sh` — line 48-51 (`SELF_TEST_COUNT` and its comment), lines 63-80 (`usage()` text), the block directly ABOVE `run_self_tests() {` (currently line 5241), the body of `run_self_tests` (add one call line after `  early_exit_reader_self_test`), line 5227 comment ("All FIFTEEN are defined above").

**Interfaces:**
- Produces: `PIPE_CAPACITY_FLOOR` (global, value `8192`); `pipe_capacity_verdict <string>` → prints exactly one of `ok`, `small`, `invalid` on stdout, always returns 0; `pipe_capacity_self_test` (a `*_self_test` table).

- [ ] **Step 1: Add the fixture table and register it (red-first)**

Insert this block directly above the line `run_self_tests() {`:

```bash
# SMA-612 — the sixteenth self-test. Pure input -> verdict rows: no pipe, no subprocess, no
# actionlint. It proves pipe_capacity_verdict, which the full-gate preflight below reads. The
# preflight itself runs only in full-gate mode, so this table is the only proof in --self-test mode.
pipe_capacity_self_test() {
  local rc=0
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  expect_pipe_capacity() {
    local name="$1" input="$2" want="$3" got
    got="$(pipe_capacity_verdict "$input")"
    if [ "$got" != "$want" ]; then
      fail "pipe-capacity self-test '$name': input '$input' gave '$got', expected '$want'. The
      full-gate preflight would misread the pipe probe."
      rc=1
    fi
  }

  expect_pipe_capacity 'the measured small state (SMA-612 M4)' '512' small
  expect_pipe_capacity 'one byte below the floor' '8191' small
  expect_pipe_capacity 'exactly the floor' '8192' ok
  expect_pipe_capacity 'the Linux default' '65536' ok
  expect_pipe_capacity 'a leading zero is decimal, not octal' '08192' ok
  expect_pipe_capacity 'nine digits is the largest accepted length' '999999999' ok
  expect_pipe_capacity 'zero' '0' invalid
  expect_pipe_capacity 'zero with a leading zero' '00' invalid
  expect_pipe_capacity 'empty output' '' invalid
  expect_pipe_capacity 'not a number' 'abc' invalid
  expect_pipe_capacity 'a negative number' '-1' invalid
  expect_pipe_capacity 'a trailing space' '512 ' invalid
  expect_pipe_capacity 'two lines' "$(printf 'abc\n65536')" invalid
  expect_pipe_capacity 'an NDJSON preamble that passed tail -n1' '{"type":"message"}' invalid
  expect_pipe_capacity 'ten digits would wrap in bash arithmetic' '1234567890' invalid

  return $rc
}
```

Then:
1. In `run_self_tests`, add the line `  pipe_capacity_self_test` directly after `  early_exit_reader_self_test`. Exactly two spaces, the name, nothing after it (check 9's awk at `run.sh:5313` accepts only this form).
2. Change line 48 to `SELF_TEST_COUNT=16  # extractor, path-filter, branch-filter, config, ci-target-floor,` and change the last line of its comment from `# release-plan, doc-diagnosis, early-exit-reader` to `# release-plan, doc-diagnosis, early-exit-reader, pipe-capacity`.

- [ ] **Step 2: Run the self-tests and see the new table fail**

Run: `/bin/bash ci/actionlint/run.sh --self-test; echo rc=$?`
Expected: rc 1. Failure rows `pipe-capacity self-test '…'` appear (the function does not exist yet, so `got` is empty). The two known `cargo-lock-step` rows also appear. No `self-test counter` row (the count is 16 and 16 ran).

- [ ] **Step 3: Add the floor and the verdict function**

Insert directly ABOVE the `pipe_capacity_self_test() {` block from Step 1:

```bash
# SMA-612 — the pipe-capacity floor and its verdict. On a host where a new pipe holds only 512
# bytes, this gate cannot finish: bash 5.x writes a here-string over 512 bytes into a pipe before
# the reader starts and deadlocks in run_self_tests, and actionlint 1.7.12 writes each run: script
# into shellcheck's stdin before it starts shellcheck (rhysd/actionlint#650) and spins forever.
# 8192 is 3.3 times the largest run: block (2498 bytes, 2026-09-19) and at or below every healthy
# value (macOS 16384, Linux 65536, the Linux soft-limit fallback 8192). The probe detects "the
# kernel does not grow a new pipe past its 512-byte minimum". It does not detect "below normal".
# See docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md.
PIPE_CAPACITY_FLOOR=8192

# Prints exactly one of ok / small / invalid. Pure: no subprocess, no file access.
# invalid: empty, any non-digit (newline and space included), more than 9 digits (bash arithmetic
# wraps), or zero. `10#` forces base 10, so `08192` is not read as octal.
pipe_capacity_verdict() {
  local v="$1"
  case "$v" in
    ''|*[!0-9]*) echo invalid; return 0 ;;
  esac
  if [ "${#v}" -gt 9 ]; then
    echo invalid; return 0
  fi
  v=$((10#$v))
  if [ "$v" -eq 0 ]; then
    echo invalid
  elif [ "$v" -lt "$PIPE_CAPACITY_FLOOR" ]; then
    echo small
  else
    echo ok
  fi
  return 0
}
```

- [ ] **Step 4: Run the self-tests and see the table pass**

Run: `/bin/bash ci/actionlint/run.sh --self-test; echo rc=$?`
Expected: rc 1, and the ONLY failure rows are the two known `cargo-lock-step` rows. No `pipe-capacity` row.

- [ ] **Step 5: Delete-the-feature checks (memory "red-first is not proof")**

Do each with the Edit tool, run, then undo with the Edit tool (never `git checkout`):
1. Change the last `echo ok` in `pipe_capacity_verdict` to `echo small`. Run Step 4's command. Expected: `pipe-capacity self-test` failure rows for every `ok` row. Undo.
2. Delete the line `  pipe_capacity_self_test` from `run_self_tests`. Run Step 4's command. Expected: a `self-test counter:` failure (15 of 16 ran). Undo.
3. Run `git diff --stat` and confirm only the intended edits remain.

- [ ] **Step 6: Update the counts in `run.sh` text**

1. `usage()` (lines 63-80): change "run the fifteen fixture tables only" to "run the sixteen fixture tables only", and change `doc-diagnosis, early-exit reader. The early-exit-reader table reads its` so that the list ends `doc-diagnosis, early-exit reader, pipe capacity.` Keep the following sentence about fixtures. Keep lines under 100 characters and keep the `echo "…" >&2` shape.
2. Line 5227 comment: "All FIFTEEN are defined above" → "All SIXTEEN are defined above".
3. Search the file: `grep -n -i 'fifteen\|sixteen' ci/actionlint/run.sh`. Update every remaining statement of the table count (15 → 16) or of the battery size (sixteen concurrent → seventeen concurrent). Do not change `SELF_TEST_COUNT` history text that names which issue added which table; add "SMA-612 the sixteenth (`pipe_capacity_self_test`)" where such a history list exists.

Run Step 4's command again. Expected: same result as Step 4.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(ci): add the pipe-capacity verdict and its fixture table to repo:actionlint (SMA-612)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: The preflight and its production call

**Files:**
- Modify: `ci/actionlint/run.sh` — add `PIPE_CAPACITY_PROBE_PY` and `pipe_capacity_preflight` directly below `pipe_capacity_verdict` (Task 1); add one line directly after the `esac` that closes the argument `case` (currently line 5402), before the check-7 comment block and `run_self_tests`.

**Interfaces:**
- Consumes: `PIPE_CAPACITY_FLOOR`, `pipe_capacity_verdict` (Task 1); the existing `infra` (`run.sh:58-61`) and `SELF_TEST_ONLY` (`run.sh:5394`).
- Produces: `pipe_capacity_preflight` (no arguments; returns only on `ok`, otherwise `infra` exits 2). The exact pinned lines listed in Step 1 — Task 3 pins them verbatim, so do not reformat them.

- [ ] **Step 1: Add the probe program and the preflight function**

Insert directly below the closing `}` of `pipe_capacity_verdict`:

```bash
# The probe: one new pipe, non-blocking write end, one byte per write until the kernel refuses
# (or 1 MiB, so the probe itself can never run forever). One byte per write measures the real
# capacity; a large write returns a partial count and would hide it (SMA-612 M5).
PIPE_CAPACITY_PROBE_PY='import fcntl, os
r, w = os.pipe()
fcntl.fcntl(w, fcntl.F_SETFL, os.O_NONBLOCK)
n = 0
try:
    while n < 1 << 20:
        n += os.write(w, b"x")
except BlockingIOError:
    pass
print(n)'

# SMA-612 — full-gate preflight. It runs BEFORE run_self_tests, because under bash 5.x the
# self-tests themselves deadlock on a small pipe. It is not run in --self-test mode: check 9
# starts one --self-test subprocess per table, and a uv run in each is the cost the SHELLCHECK_BIN
# comment below already refuses. Every failure is infra (rc 2): no check ran, so rc 1 would be a
# false "a workflow is wrong" and rc 0 a false pass. `ok)` is the only arm that continues.
pipe_capacity_preflight() {
  local pc_out pc_rc=0
  grep -qxF 'actionlint = "1.7.12"' .prototools || infra ".prototools no longer pins actionlint 1.7.12. The pipe-capacity probe (SMA-612) exists only for that version. Re-decide it per docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md D9."
  pc_out="$(uv run --locked --project py python3 -c "$PIPE_CAPACITY_PROBE_PY" | tail -n1)" || pc_rc=$?
  [ "$pc_rc" -eq 0 ] || infra "the pipe-capacity probe failed (rc $pc_rc) via 'uv run --locked --project py'. No check ran."
  case "$(pipe_capacity_verdict "$pc_out")" in
    ok) echo "actionlint gate: pipe capacity $pc_out bytes (floor $PIPE_CAPACITY_FLOOR)" >&2 ;;
    small) infra "a new pipe on this host holds only $pc_out bytes (floor $PIPE_CAPACITY_FLOOR). With pipes this small, the gate cannot finish: bash 5.x here-strings in its own self-tests deadlock above $pc_out bytes, and the pinned actionlint writes each run: script into shellcheck's stdin before it starts shellcheck (rhysd/actionlint#650). No check ran. See ci/actionlint/README.md, \"Small pipes on macOS\"." ;;
    *) infra "the pipe-capacity probe printed '$pc_out', not a positive integer. No check ran." ;;
  esac
}
```

Notes for the implementer:
- `grep -qxF … .prototools` reads a FILE, not a pipe, so check 13 does not apply.
- `local pc_out pc_rc=0` is on its own line, and the capture is `… || pc_rc=$?`, per the CLAUDE.md SMA-647 rule. Under `pipefail` the capture's status is `uv`'s status, not `tail`'s.
- `infra` inside this function exits the whole script. That is correct here: the function is called from the main shell, not from a `done < <(...)` subshell.
- `.prototools` is read relative to the repo root; `run.sh` already does `cd "$(git rev-parse --show-toplevel)"` at line 38.

- [ ] **Step 2: Add the production call line**

Directly after the `esac` that closes `case "$#:${1:-}" in` (currently line 5402), insert a blank line and then:

```bash
# SMA-612 — the pipe-capacity preflight, full-gate mode only, before any self-test. Column 0 and
# unwrapped on purpose: ci/affected-graph/ci_targets.py pins this exact line at column 0.
[ "$SELF_TEST_ONLY" = 1 ] || pipe_capacity_preflight
```

- [ ] **Step 3: Full gate on this host, both bashes**

Run each with a hard time limit. Use a scratchpad script like this (adjust `B`):

```bash
#!/bin/bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-612-shellcheck-hang || exit 2
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
B=$1; start=$(date +%s)
"$B" ci/actionlint/run.sh > "${TMPDIR:-/tmp}/sma612-gate.out" 2>&1 &
pid=$!
while kill -0 "$pid" 2>/dev/null && [ $(( $(date +%s) - start )) -lt 60 ]; do sleep 1; done
if kill -0 "$pid" 2>/dev/null; then echo "HANG under $B"; kill -9 "$pid"; else wait "$pid"; echo "rc=$? in $(( $(date +%s) - start ))s under $B"; fi
cat "${TMPDIR:-/tmp}/sma612-gate.out"
ps -A -o pid=,command= | grep -E '[a]ctionlint -|[s]hellcheck ' || echo "no actionlint/shellcheck left"
```

Run it with `/bin/bash` and with `/opt/homebrew/bin/bash`.
Expected for BOTH: `rc=2` in a few seconds; the output is exactly one line starting `actionlint gate: INFRASTRUCTURE ERROR: a new pipe on this host holds only 512 bytes (floor 8192).`; no self-test rows before it; `no actionlint/shellcheck left`. Record both times in the task report.

- [ ] **Step 4: `--self-test` mode does not probe**

Run: `/bin/bash ci/actionlint/run.sh --self-test; echo rc=$?`
Expected: identical to Task 1 Step 4 (rc 1, only the two `cargo-lock-step` rows). No `pipe capacity` or `INFRASTRUCTURE ERROR` line.

- [ ] **Step 5: Failure paths (temporary edits, undone with the Edit tool)**

Use the Step 3 script with `/bin/bash` for each:
1. In `pipe_capacity_preflight`, temporarily replace `python3 -c "$PIPE_CAPACITY_PROBE_PY"` with `python3 -c "print('abc')"`. Expected: rc 2, message `the pipe-capacity probe printed 'abc', not a positive integer. No check ran.` Undo.
2. Temporarily replace `python3 -c "$PIPE_CAPACITY_PROBE_PY"` with `python3 -c "import sys; sys.exit(3)"`. Expected: rc 2, message `the pipe-capacity probe failed (rc 3) …`. Undo.
3. Temporarily change `.prototools` line `actionlint = "1.7.12"` to `actionlint = "1.7.13"`. Expected: rc 2, the `.prototools no longer pins actionlint 1.7.12` message. Undo.
4. Temporarily set `PIPE_CAPACITY_FLOOR=512`. Expected: the gate passes the preflight, prints `actionlint gate: pipe capacity 512 bytes (floor 512)`, prints `pipe-capacity self-test` failure rows (the fixture rows now disagree with the floor), and then HANGS in check 1 (the Step 3 script reports `HANG` after 60 s) — this proves the probe is what stops the hang. `kill -9` on bash does NOT stop its `actionlint` child, which keeps spinning: afterwards run `pkill -9 -f 'actionlint -pyflakes='` and confirm with `ps -A -o pid=,command= | grep '[a]ctionlint -'` that nothing is left. Undo the floor edit.
5. `git diff --stat` shows only the Task 2 edits in `ci/actionlint/run.sh`, and `.prototools` is unchanged.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "fix(ci): stop repo:actionlint hanging on a host with 512-byte pipes (SMA-612)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Pin the preflight in `ci_targets.py`

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` — `ACTIONLINT_SH_CALL_SITES` (starts line 798), `ACTIONLINT_SH_INDENTED_CALL_SITES` (starts line 972), the `wired_actionlint` fixture (starts line 2456), and the deletion/indentation rows (the check-13 rows end near line 2852).

**Interfaces:**
- Consumes: the exact lines from Task 2 Step 1 and Step 2. They must match byte for byte (after strip for the indented ones).
- Produces: module constants `PIPE_CAPACITY_CALL_SITES` (1 entry, column 0) and `PIPE_CAPACITY_INDENTED_CALL_SITES` (6 entries).

- [ ] **Step 1: Add the two constants and splat them into the pin tables**

Directly ABOVE the comment block that precedes `ACTIONLINT_SH_CALL_SITES = (` (the block starting `# COLUMN 0, not stripped-both-sides`), add:

```python
# SMA-612 — the pipe-capacity preflight in ci/actionlint/run.sh. Its call line sits at run.sh's
# top level and is pinned at COLUMN 0 (ACTIONLINT_SH_CALL_SITES). The six lines inside
# pipe_capacity_preflight() carry real leading whitespace, so they are pinned stripped
# (ACTIONLINT_SH_INDENTED_CALL_SITES). The check-10 precedent pins the call, the status capture
# and the routing arms for the same reason: a one-token edit to any one of them (a constant in
# place of the capture, an empty `small)` arm, a deleted status guard) disables the preflight
# while every other pinned line stays byte-identical. Kept as named tuples so the self-test below
# can drive its deletion and indentation rows from them.
PIPE_CAPACITY_CALL_SITES = (
    '[ "$SELF_TEST_ONLY" = 1 ] || pipe_capacity_preflight',
)
PIPE_CAPACITY_INDENTED_CALL_SITES = (
    "grep -qxF 'actionlint = \"1.7.12\"' .prototools || infra \".prototools no longer pins actionlint 1.7.12. The pipe-capacity probe (SMA-612) exists only for that version. Re-decide it per docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md D9.\"",
    'pc_out="$(uv run --locked --project py python3 -c "$PIPE_CAPACITY_PROBE_PY" | tail -n1)" || pc_rc=$?',
    '[ "$pc_rc" -eq 0 ] || infra "the pipe-capacity probe failed (rc $pc_rc) via \'uv run --locked --project py\'. No check ran."',
    'case "$(pipe_capacity_verdict "$pc_out")" in',
    'small) infra "a new pipe on this host holds only $pc_out bytes (floor $PIPE_CAPACITY_FLOOR). With pipes this small, the gate cannot finish: bash 5.x here-strings in its own self-tests deadlock above $pc_out bytes, and the pinned actionlint writes each run: script into shellcheck\'s stdin before it starts shellcheck (rhysd/actionlint#650). No check ran. See ci/actionlint/README.md, \\"Small pipes on macOS\\"." ;;',
    '*) infra "the pipe-capacity probe printed \'$pc_out\', not a positive integer. No check ran." ;;',
)
```

Then add `*PIPE_CAPACITY_CALL_SITES,` as the LAST element of `ACTIONLINT_SH_CALL_SITES`, and `*PIPE_CAPACITY_INDENTED_CALL_SITES,` as the LAST element of `ACTIONLINT_SH_INDENTED_CALL_SITES`, each with a one-line comment `# SMA-612 — see PIPE_CAPACITY_CALL_SITES above.`

Verify each entry against the real file (whitespace-stripped for the indented ones) with a one-off check:

```bash
python3 - <<'EOF'
import importlib.util, pathlib
spec = importlib.util.spec_from_file_location("ct", "ci/affected-graph/ci_targets.py")
ct = importlib.util.module_from_spec(spec); spec.loader.exec_module(ct)
text = pathlib.Path("ci/actionlint/run.sh").read_text()
col0 = {l.rstrip() for l in text.splitlines() if l == l.lstrip()}
stripped = {l.strip() for l in text.splitlines()}
for s in ct.PIPE_CAPACITY_CALL_SITES: print("col0", s in col0, s[:60])
for s in ct.PIPE_CAPACITY_INDENTED_CALL_SITES: print("indented", s in stripped, s[:60])
EOF
```
Expected: seven lines, all `True`. If one is `False`, fix the Python string escaping, never the bash line.

- [ ] **Step 2: Run the real check**

Run: `python3 ci/affected-graph/ci_targets.py; echo rc=$?`
Expected: rc 0, `PASS  ci-targets`.

- [ ] **Step 3: Add the lines to `wired_actionlint` and run the self-test (it reds first)**

Run: `python3 ci/affected-graph/ci_targets.py --self-test; echo rc=$?`
Expected BEFORE the fixture edit: rc 1, a `check_self_invocation: fired on a wired tree` failure that names the seven new sites.

Now append to the end of the `wired_actionlint = ( … )` string, after the `'done < <(early_exit_reader_verdict "$EE_LIST")\n'` line:

```python
        # SMA-612 — the pipe-capacity preflight: its column-0 call line, then the six lines
        # inside pipe_capacity_preflight(), indented two spaces as the real function body is.
        # Derived from the registry so the fixture cannot drift from the pin it exercises.
        + "".join(f"{site}\n" for site in PIPE_CAPACITY_CALL_SITES)
        + "".join(f"  {site}\n" for site in PIPE_CAPACITY_INDENTED_CALL_SITES)
```

Note: the existing `wired_actionlint` is a parenthesised run of adjacent string literals. Adjacent-literal concatenation binds before `+`, so appending `+ "".join(...)` terms inside the same parentheses is valid. If ruff or the reader objects, close the literal run with `)` and write `wired_actionlint = (…) + "".join(…) + "".join(…)`.

Run the self-test again. Expected: rc 0.

- [ ] **Step 4: Add the deletion and indentation rows**

Directly after the check-13 indentation row (the block ending `"check_self_invocation: an INDENTED check-13 call site satisfied the column-0 pin"` and its closing `)`), add:

```python
    # SMA-612 — the pipe-capacity preflight's seven pinned lines, each deleted in turn, and its
    # column-0 call line INDENTED. Same shape as the check-13 rows above. The count assertions
    # make a line dropped from either registry red here as well.
    if len(PIPE_CAPACITY_CALL_SITES) != 1 or len(PIPE_CAPACITY_INDENTED_CALL_SITES) != 6:
        failures.append(
            "check_self_invocation: expected 1 column-0 and 6 indented SMA-612 pipe-capacity "
            f"entries, found {len(PIPE_CAPACITY_CALL_SITES)} and "
            f"{len(PIPE_CAPACITY_INDENTED_CALL_SITES)}"
        )
    for _pc_line in (
        *(f"{site}\n" for site in PIPE_CAPACITY_CALL_SITES),
        *(f"  {site}\n" for site in PIPE_CAPACITY_INDENTED_CALL_SITES),
    ):
        _pc_broken = wired_actionlint.replace(_pc_line, "")
        if _pc_broken == wired_actionlint:
            failures.append(
                f"check_self_invocation: wired_actionlint lacks the SMA-612 line {_pc_line!r}"
            )
        elif not check_self_invocation(wired, scripts, _pc_broken, wired_release_parity, wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free):
            failures.append(
                f"check_self_invocation: missed a deleted SMA-612 line {_pc_line!r}"
            )
    for _pc_site in PIPE_CAPACITY_CALL_SITES:
        indented_pc_call = wired_actionlint.replace(f"{_pc_site}\n", f"  {_pc_site}\n")
        if not check_self_invocation(wired, scripts, indented_pc_call, wired_release_parity, wired_workflow_credentials, wired_release_plan, wired_ruff, wired_next_public_free):
            failures.append(
                "check_self_invocation: an INDENTED SMA-612 preflight call satisfied the "
                "column-0 pin"
            )
```

Run: `python3 ci/affected-graph/ci_targets.py --self-test; echo rc=$?` → Expected rc 0.
Run: `python3 ci/affected-graph/ci_targets.py; echo rc=$?` → Expected rc 0.

- [ ] **Step 5: Prove the pin against the real file (spec §7 test 6)**

With the Edit tool, delete the line `[ "$SELF_TEST_ONLY" = 1 ] || pipe_capacity_preflight` from `ci/actionlint/run.sh`. Run `python3 ci/affected-graph/ci_targets.py; echo rc=$?`. Expected: rc 1 and a row naming that line. Restore it with the Edit tool. Repeat once for the `small) infra …` line. Run again: rc 0. `git diff --stat ci/actionlint/run.sh` → empty.

- [ ] **Step 6: Lint**

Run: `uv run --locked --project py ruff check ci/affected-graph/ci_targets.py; echo rc=$?`
Expected: rc 0. (This is what `repo:ruff-ci` runs; line length is 200.)

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "test(ci): pin the repo:actionlint pipe-capacity preflight in ci_targets.py (SMA-612)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `ci/actionlint/README.md`, `moon.yml` (the `repo:actionlint` comment block, lines 680-730), `CLAUDE.md`.

**Interfaces:**
- Consumes: the facts in spec §2 (M1–M10), §3, §9. Copy numbers exactly.

- [ ] **Step 1: README — new section "Small pipes on macOS"**

Add a `## Small pipes on macOS` section to `ci/actionlint/README.md` (before the Limitations section). Content, in Simplified Technical English:
1. The symptom: the gate ran for many minutes with no output on the development Mac.
2. The trigger: a new pipe on that host holds 512 bytes (M4, M6, M10).
3. Mechanism 1 (actionlint 1.7.12 writes stdin before it starts shellcheck; busy loop at about 300% CPU; upstream #650, fix `fd33e9f582`, not in a release on 2026-09-19).
4. Mechanism 2 (bash 5.x here-strings; the measured boundary 512 works, 513 hangs, M9; the self-tests deadlock at about 0% CPU, M8).
5. What the gate does now: the full-gate preflight exits rc 2 with the `small` message. `--self-test` does not probe.
6. How to check a host: the one-line Python command from spec §10 Q1.
7. The syntax-only workaround `actionlint -shellcheck= -pyflakes= <file>`, and the plain statement that it is NOT the gate: it skips shellcheck and every other check.
8. What removes the probe: spec D9 and the follow-up issue (write "the SMA-612 follow-up issue"; the controller adds its key later).

- [ ] **Step 2: README — Limitations and counts**

1. Add a Limitations entry (next free L-number) for spec R1–R4, one short paragraph each.
2. Add a paragraph "SMA-612 added a SIXTEENTH self-test (`pipe_capacity_self_test`)" beside the existing per-issue self-test paragraphs.
3. Update every count: `grep -n -i 'fifteen\|sixteen\|15 \|16 ' ci/actionlint/README.md`. Known places: lines 38 (check 7 row: list the new table, "all sixteen ran"), 45 ("each of the sixteen self-test invocations"), 755-757 ("seventeen times per full gate run — sixteen mutants plus the unmutated control", and "roughly 17x"), 774-777 ("State: CURRENT — sixteen fixture tables, sixteen mutants (seventeen concurrent --self-test subprocesses …)"), 785-793 (the full-gate path now has TWO `uv run` calls: the preflight probe and `SHELLCHECK_BIN`), 802 and 805 ("the sixteen fixture tables only").

- [ ] **Step 3: `moon.yml` comment counts**

In the `repo:actionlint` comment block: "FIFTEEN fixture tables" → "SIXTEEN fixture tables", add `pipe capacity` to the list, add "and SMA-612 the sixteenth (`pipe_capacity_self_test`)" to the history sentence, and "SIXTEEN concurrent subprocesses (fifteen mutants …)" → "SEVENTEEN concurrent subprocesses (sixteen mutants …)", with "fifteen until SMA-612 added the pipe-capacity table" appended to that history. Change comments only; do not touch any YAML key or the `script:` line.

- [ ] **Step 4: CLAUDE.md**

1. Add one new Gotchas bullet, directly after the "LOCAL ONLY, CORRECTED (SMA-512)" bullet:
   - Title in bold: **A new pipe on the development Mac can hold only 512 bytes, and two local hangs come from it** (SMA-612).
   - State M4 (512 bytes; normal macOS is 16384; the pipe does not grow, M6; the sandbox is not the cause, M10).
   - Mechanism 1 and 2 in two sentences each, with M8 and M9 numbers.
   - `repo:actionlint` now runs a pipe-capacity preflight before its self-tests in full-gate mode and exits rc 2 with a `small` message. `--self-test` does not probe and still hangs under Homebrew bash on such a host.
   - Remove the probe only per spec D9; a `.prototools` actionlint bump reds the gate on purpose until then.
   - Other gates that use here-strings (for example `repo:affected-smoke`) still hang under bash 5.x on such a host; this entry does not fix them.
2. In the "LOCAL ONLY, CORRECTED (SMA-512)" bullet, add one sentence after "no local bash currently runs `ci/actionlint/run.sh` to completion": "SMA-612 found the cause of both hangs: a 512-byte pipe (see the next entry). The full gate now exits rc 2 in seconds under either bash instead of hanging."
3. In the `repo:actionlint` / `repo:affected-smoke` guard bullet: "Adding a sixteenth-and-later `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 15" → "Adding a seventeenth-and-later `*_self_test` table means bumping `SELF_TEST_COUNT` (currently 16", and after "and SMA-647 the fifteenth, `early_exit_reader_self_test` at check 13" add ", and SMA-612 the sixteenth, `pipe_capacity_self_test` (the full-gate preflight)".

- [ ] **Step 5: Verify the doc gates do not red**

1. `python3 ci/affected-graph/ci_targets.py; echo rc=$?` → rc 0 (it reads CLAUDE.md's target block).
2. `grep -c 'ci-targets:begin' CLAUDE.md` → `1`.
3. `grep -n "$(printf 'ci%s' 'Report')" CLAUDE.md ci/actionlint/README.md docs/superpowers/specs/2026-09-19-sma-612-actionlint-pipe-capacity-design.md docs/superpowers/plans/2026-09-19-sma-612-actionlint-pipe-capacity.md | grep -c SMA-612` → `0` (no new mention of the report file in SMA-612 text).
4. `/bin/bash ci/actionlint/run.sh --self-test; echo rc=$?` → only the two known `cargo-lock-step` rows.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/README.md moon.yml CLAUDE.md
git commit -m "docs(ci): record the 512-byte pipe cause of the local actionlint hangs (SMA-612)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5 (controller only, not a subagent): memory and the follow-up issue

- [ ] Update memory files `macos-bash-535-herestring-deadlock.md`, `affected-smoke-hang-fixed-by-system-bash.md` and `gate-failures-from-wrong-bash.md`, and their `MEMORY.md` index lines, with the measured 512-byte boundary (M9) and the common trigger.
- [ ] Create the Linear follow-up issue: "ci: bump actionlint past 1.7.12 and re-decide the SMA-612 pipe-capacity probe", related to SMA-612, body = spec D9 plus the removal list (probe, self-test, `SELF_TEST_COUNT` 16 → 15, the seven `ci_targets.py` pins, the `wired_actionlint` lines, the deletion and indentation rows, every doc count).
- [ ] Put the follow-up key into `ci/actionlint/README.md` ("the SMA-612 follow-up issue" → the key) and commit: `docs(ci): link the SMA-612 follow-up issue`.
- [ ] Final check: `python3 ci/affected-graph/ci_targets.py --self-test`, `python3 ci/affected-graph/ci_targets.py`, `/bin/bash ci/actionlint/run.sh --self-test`, and Task 2 Step 3 under both bashes, all with the expected results.
