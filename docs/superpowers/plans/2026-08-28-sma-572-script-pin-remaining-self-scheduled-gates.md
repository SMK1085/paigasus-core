# SMA-572 — script-pin the remaining self-scheduled gates: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every gate that runs its own self-test from `moon.yml` unable to lose that
invocation silently, and make `repo:affected-smoke`'s `inputs` — the list that schedules every
pin in `ci/affected-graph/ci_targets.py` — unable to shrink silently either.

**Architecture:** Two independently scheduled gates guard each other. `ci_targets.py` (runs
inside `repo:affected-smoke`) pins four gates' `moon.yml` script lines and three gates' exact
input sets, including `repo:actionlint`'s `['**/*']`. A new check 8e in `ci/actionlint/run.sh`
(`inputs: ['**/*']`, so it runs on every PR) pins `repo:affected-smoke`'s own inputs by
containment and its script lines in order. Neither gate is the sole judge of itself.

**Tech Stack:** Python 3 (stdlib only), Bash + awk, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-28-sma-572-script-pin-remaining-self-scheduled-gates-design.md`

## Global Constraints

- Every source file opens with an SPDX header (`# SPDX-License-Identifier: Apache-2.0` for
  Python and Bash). Both files edited here already have one; do not add a second.
- `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before any `moon`/`uv`/`buf`/`nextest`
  command — the Bash tool's PATH lacks the proto-managed CLIs, and shims must come first.
- Restore a mutated file by **reverting the edit** (Edit tool or `git checkout --`), never by
  moving a `.bak` file back: that rolls mtime backwards and a cached PASS replays over the
  restored tree.
- Where Moon caching could serve a stale result, invoke the script directly or pass `--force`.
- Commit messages: conventional commits with a workspace scope (`ci:` here). Subject must start
  **lowercase** and be ≤100 chars. No `#NNN` issue refs in the body — write "owner/repo PR NNN".
  Keep one contiguous footer block.
- `ci/actionlint/run.sh` targets the same bash the repo already assumes: **indexed arrays only,
  no associative arrays** (`declare -A`), mirroring `COE_SKIP` / `SWALLOWED_SKIP` / `BRANCH_SKIP`.
- Do **not** touch `T` in `.github/workflows/ci.yml` or CLAUDE.md's marker-delimited command.
  Adding a second copy of either marker anywhere in CLAUDE.md reds `repo:affected-smoke`.

## Superseded during execution

This plan is a historical execution artifact; the steps below are left as originally written,
but were overridden by rulings made during execution:

- **Task 2, Step 6's expected message is wrong.** Deleting a self-test *invocation* while
  keeping its *definition* leaves definitions == `SELF_TEST_COUNT`, so the definition-count
  check passes. What actually fires under `--self-test` is `assert_self_tests_ran` (N ran, N+1
  expected); under a full run, check 9's invocation-count precondition also fires.
- **Task 3 pins THREE `ACTIONLINT_SH_CALL_SITES` entries, not two.** The
  `T_AFFECTED_SMOKE_REQUIRED_SCRIPT` arity floor was added during review: without it, deleting
  that floor and emptying the script table in one edit silently drops check 8e's script-line
  ORDER assertion — the one property `ci_targets.py`'s `SELF_SCHEDULED_GATES` cannot cover,
  since it compares an unordered set.
- **Tasks 4 and 5 were executed in the order 5 then 4.** Task 5 edits `moon.yml` and
  `CLAUDE.md`, both inputs to `repo:affected-smoke`; running Task 4's acceptance evidence first
  would have produced evidence for a tree that is not the one that shipped.
- **Task 4's Step 1 expectation was stale** and was replaced by a direct measurement (an
  emptied required-inputs table produces zero verdicts against the live `moon.yml`), because
  several fixture rows hard-code literal globs and notice an emptied array incidentally.
- **The cost budget was NOT met.** The design's "reconsider above baseline + 10%" trigger
  fired; the regression was measured, partly recovered by making the verdict fork-free, and the
  remainder explicitly accepted. See `ci/actionlint/README.md`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `ci/affected-graph/ci_targets.py` | The registries: which gates are script-pinned, whose inputs are exact-pinned, which are exempt, and which lines of `ci/actionlint/run.sh` must exist | Modify (Tasks 1, 3) |
| `ci/actionlint/run.sh` | Check 8e: the `moon.yml` extractor, the required-input/script tables, their verdict function, its fixture table, and the production call site | Modify (Tasks 2, 3) |
| `ci/affected-graph/README.md` | C4's list of pinned gates and the pairing rule | Modify (Task 5) |
| `ci/actionlint/README.md` | Check 8e's contract and residuals | Modify (Task 5) |
| `ci/release-parity/README.md` | L1, closed by this change | Modify (Task 5) |
| `CLAUDE.md` | The `SELF_SCHEDULED_GATES` gotcha's line counts | Modify (Task 5) |
| `moon.yml` | Comments quoting "nine fixture tables" / "ten concurrent subprocesses" | Modify (Task 5) |

---

## Task 1: Registries in `ci_targets.py`

Implements spec §1 and §2. Adds four `SELF_SCHEDULED_GATES` entries, three exact
`SELF_TASK_EXPECTED_GLOBS` entries, and one `SELF_TASK_GLOBS_EXEMPT` entry.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_TASK_EXPECTED_GLOBS` ~184,
  `SELF_SCHEDULED_GATES` ~265, `SELF_TASK_GLOBS_EXEMPT` ~336)
- Test: the file's own `--self-test` path; no separate test file exists in this repo.

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `SELF_SCHEDULED_GATES` keys `"affected-smoke"`, `"publish-metadata"`,
  `"error-code-single-site"`, `"actionlint"`; `SELF_TASK_EXPECTED_GLOBS` keys
  `"publish-metadata"`, `"error-code-single-site"`, `"actionlint"`;
  `SELF_TASK_GLOBS_EXEMPT["affected-smoke"]`. Task 3 adds two entries to
  `ACTIONLINT_SH_CALL_SITES` and one line to `self_test()`'s `wired_actionlint` fixture.

- [ ] **Step 1: Confirm the live values the pins must match**

The registries must match what moon actually resolves, not what `moon.yml` appears to say.
Run this first and keep the output — Steps 3 and 4 copy from it verbatim.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks > /tmp/qt.json
python3 - <<'PY'
import json
d = json.load(open('/tmp/qt.json'))['tasks']['repo']
INJECTED = ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}"
for t in ['actionlint', 'publish-metadata', 'error-code-single-site', 'affected-smoke']:
    e = d[t]
    globs = tuple(g for g in sorted(e.get('inputGlobs') or {}) if g != INJECTED)
    files = tuple(sorted(e.get('inputFiles') or {}))
    print(f'{t}:')
    print(f'  script  = {e.get("script")!r}')
    print(f'  expected = {globs + files}')
PY
```

Expected: `actionlint`'s script is the single line `'ci/actionlint/run.sh'`; the other three are
three-line scripts starting `set -euo pipefail\n`. If any value differs from what Steps 3-4
encode, use the live value and note the discrepancy — a stale pin is a false red on every PR.

- [ ] **Step 2: Write the failing check — prove the registries are currently unpinned**

There is no separate test file; the assertion lives in the module's own self-test, driven by
`pairing("real-registries", ...)` at `ci_targets.py:1658`. So the "failing test" here is a
demonstration that a deleted control line is currently invisible. Record its output.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import json, subprocess
out = subprocess.run(["moon", "query", "tasks"], capture_output=True, text=True, check=True).stdout
import sys; sys.path.insert(0, "ci/affected-graph")
import ci_targets
scripts = ci_targets._scripts(json.loads(out)["tasks"])
# Simulate deleting the negative-control line from repo:publish-metadata's script.
scripts["publish-metadata"] = scripts["publish-metadata"].replace(
    "bash ci/publish-metadata/run.sh --negative-control\n", "")
missing = ci_targets.check_self_invocation(
    open("ci/affected-graph/run.sh").read(), scripts,
    open("ci/actionlint/run.sh").read(), open("ci/release-parity/run.sh").read())
print("missing:", missing)
PY
```

Expected: `missing: []` — the deletion is invisible. That is the defect.

- [ ] **Step 3: Add the four `SELF_SCHEDULED_GATES` entries**

Append inside the existing dict, after the `"version-lockstep"` entry, keeping the file's
comment-heavy house style:

```python
    # SMA-572 — the three gates SMA-530 left out, plus repo:actionlint. Same three-line shape
    # and the same reasoning: Moon takes a `script:` block's status from its LAST command, so
    # `set -euo pipefail` is exactly as load-bearing as either invocation. Whole-line matched,
    # which is what makes the first two safe — `bash ci/publish-metadata/run.sh` is a strict
    # PREFIX of its own --negative-control line, so a substring test would report the script
    # fully wired after the REAL RUN had been deleted.
    #
    # repo:affected-smoke's third entry has NO TRUE-POSITIVE COVERAGE and that is deliberate:
    # any state in which the bare `ci/affected-graph/run.sh` line is absent is a state in which
    # THIS function never runs (run.sh:405-409 exits inside the --negative-control branch,
    # before run_suite at :412). Its real enforcement is check 8e in ci/actionlint/run.sh,
    # which is scheduled independently. It is kept here so the table's contract stays "every
    # line, one rule" — do not read it as coverage.
    "affected-smoke": (
        "set -euo pipefail",
        "ci/affected-graph/run.sh --negative-control",
        "ci/affected-graph/run.sh",
    ),
    "publish-metadata": (
        "set -euo pipefail",
        "bash ci/publish-metadata/run.sh --negative-control",
        "bash ci/publish-metadata/run.sh",
    ),
    # No prefix hazard here (--self-test and --single-site are distinct suffixes), but matched
    # whole-line like every other entry: the table's contract is one rule, not per-entry rules.
    "error-code-single-site": (
        "set -euo pipefail",
        "python3 ci/error-registry/check.py --self-test",
        "python3 ci/error-registry/check.py --single-site",
    ),
    # One command, so the script's status IS its status — there is no pipefail line to pin.
    # Registered mainly so its `inputs` pin below is not an orphan_globs row: repo:actionlint's
    # `['**/*']` is the premise that every check in ci/actionlint/run.sh (8, 8b, 8c, 8d and the
    # new 8e) runs on every PR, and until SMA-572 nothing asserted it. Narrowing it to
    # `.github/workflows/**` was a green edit that silently switched all five off.
    "actionlint": (
        "ci/actionlint/run.sh",
    ),
```

- [ ] **Step 4: Add the three exact `SELF_TASK_EXPECTED_GLOBS` entries and the exemption**

In `SELF_TASK_EXPECTED_GLOBS`, after `"version-lockstep"` (globs sorted, then literal files
sorted — the order `check_gate_inputs` compares in):

```python
    # SMA-572. repo:actionlint's whole authored declaration, and the reason the rest of this
    # file's pins are reachable at all. Checked from ci_targets.py, which runs inside
    # repo:affected-smoke — a DIFFERENT gate — so this is not self-judging: narrowing
    # repo:actionlint's inputs is a root-moon.yml edit, which schedules affected-smoke.
    "actionlint": ("**/*",),
    # SMA-572. These two were exempted in this design's first draft on the grounds that
    # repo:input-liveness asserts declared-glob liveness generically. That is wrong, and an
    # adversarial review caught it: task_inputs.py asserts a DECLARED glob still matches a
    # tracked file — it cannot see a REMOVED DECLARATION. Both lists carry entries whose
    # deletion moon.yml itself documents as fatal: publish-metadata's
    # .github/workflows/security-scan.yml ("Check 4 ASSERTS ON IT"; moon.yml:520-521) and
    # error-code's broad rs/crates/**/src/**/*.rs ("the one case it exists for would be the one
    # case it never runs on"; moon.yml:628-630). Both sets are STATIC — no runtime discovery —
    # so exact match is affordable, exactly as for version-lockstep's sixteen.
    "publish-metadata": (
        "rs/crates/**/*",
        ".github/workflows/security-scan.yml",
        ".gitignore",
        "ci/publish-metadata/categories.py",
        "ci/publish-metadata/crates-io-categories.txt",
        "ci/publish-metadata/run.sh",
        "rs/.cargo/config.toml",
        "rs/Cargo.lock",
        "rs/Cargo.toml",
        "rs/release-plz.toml",
        "rs/rust-toolchain.toml",
    ),
    "error-code-single-site": (
        "ci/error-registry/**/*",
        "rs/crates/**/src/**/*.rs",
        "contracts/proto/paigasus/common/v1/error.proto",
    ),
```

In `SELF_TASK_GLOBS_EXEMPT`, after the `release-parity-ts` entry:

```python
    # SMA-572/SMA-573. NOT a skip — a delegation, and the harder half of this issue. This gate's
    # nineteen inputs are the most load-bearing list in the repo (every pin in this file is
    # reachable only because it lists `moon.yml`), so they ARE pinned — by check 8e in
    # ci/actionlint/run.sh, which is scheduled independently of this gate. An entry in
    # SELF_TASK_EXPECTED_GLOBS instead would make repo:affected-smoke the sole judge of its own
    # reachability, which is the exact defect SMA-573 exists to close; it would also be an exact
    # match against a list that legitimately grows every time a gate keys on a new directory.
    # ACTIONLINT_SH_CALL_SITES pins check 8e's production call site AND its table's arity floor,
    # so this delegation cannot rot silently.
    "affected-smoke": (
        "inputs pinned by check 8e in ci/actionlint/run.sh instead — an entry here would make "
        "this gate the sole judge of its own reachability, and exact-match a nineteen-entry "
        "list that legitimately grows; ACTIONLINT_SH_CALL_SITES pins 8e's call site and arity "
        "floor so the delegation cannot rot (SMA-572/SMA-573)"
    ),
```

- [ ] **Step 5: Run the self-test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: exits 0. A non-zero exit naming `check_registry_pairing[real-registries]` means an
entry is unpaired; one naming `check_gate_inputs` means a tuple does not match Step 1's output.

- [ ] **Step 6: Re-run Step 2's demonstration — it must now fire**

Re-run the Step 2 snippet unchanged.

Expected: `missing: ['publish-metadata script: bash ci/publish-metadata/run.sh --negative-control']`
instead of `[]`.

- [ ] **Step 7: Run the whole gate against the real tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control && ci/affected-graph/run.sh
```

Expected: both exit 0.

- [ ] **Step 8: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "ci: script-pin publish-metadata, error-code, affected-smoke and actionlint (SMA-572)"
```

---

## Task 2: Check 8e's extractor, tables and fixture table

Implements spec §3 up to but not including the production call site. Deliverable: a self-tested
verdict function that is defined and exercised by `--self-test`, but not yet applied to the real
`moon.yml`. Task 3 applies it.

**Files:**
- Modify: `ci/actionlint/run.sh` — `SELF_TEST_COUNT` (line 40) and its inline name list;
  new definitions placed immediately after `affected_graph_wiring_verdict` (~line 2065);
  new self-test placed after `affected_graph_wiring_self_test` (~line 2692); one new call
  inside `run_self_tests` (~line 3446); the `usage()` text (~line 56).
- Test: `ci/actionlint/run.sh --self-test`.

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `T_AFFECTED_SMOKE_REQUIRED_INPUTS` (indexed array, 19 entries),
  `T_AFFECTED_SMOKE_REQUIRED_SCRIPT` (indexed array, 3 entries), `REQUIRED_INPUT_SKIP`
  (indexed array, empty), `is_required_input_skipped <glob>` (rc 0 if skipped),
  `affected_smoke_block_extract <file>` (emits `INPUT\t…` / `SCRIPT\t…` / `ERR\t…`),
  `affected_smoke_block_verdict <file>` (emits one verdict token per problem, nothing when
  clean), `affected_smoke_block_self_test`. Task 3 pins the production call site and arity
  floor by their exact text.

- [ ] **Step 1: Write the failing fixture table**

Insert after `affected_graph_wiring_self_test`'s closing brace (~line 2692). Note it increments
`SELF_TESTS_RAN` like every sibling, and asserts **both** directions — a table whose rows all
fire cannot tell a working check from a stuck one (`run.sh:2065-2066`, SMA-466).

```bash
affected_smoke_block_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmp got saved_skip

  expect_block() {
    local name="$1" expected="$2" body="$3"
    tmp="$(mktemp)"
    printf '%s' "$body" > "$tmp"
    got="$(affected_smoke_block_verdict "$tmp")"
    rm -f "$tmp"
    if [ "$got" != "$expected" ]; then
      fail "affected-smoke-block self-test '$name': got '$got', expected '$expected'. Check 8e
      is not deciding what it is documented to decide."
      rc=1
    fi
  }

  # A synthetic block wired with EXACTLY the required sets and nothing else. Built from the
  # live arrays rather than spelled out, so a gate added to either table tomorrow cannot leave
  # this control passing for the wrong reason (the vacuity SMA-530 measured on wired_scripts()).
  local wired glob line
  wired='tasks:
  affected-smoke:
    description: '"'"'x'"'"'
    script: |
'
  for line in "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}"; do wired="$wired      $line
"; done
  wired="$wired    toolchain: '"'"'system'"'"'
    inputs:
"
  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do wired="$wired      - '"'"'$glob'"'"'
"; done
  wired="$wired
  other-task:
    script: '"'"'true'"'"'
"

  expect_block 'a fully wired block is clean' '' "$wired"

  # Each required input deleted in turn. Driven from the array so a nineteenth-and-later entry
  # is covered automatically.
  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    expect_block "required input '$glob' deleted fires" "missing-input $glob" \
      "$(printf '%s' "$wired" | grep -vxF -e "      - '$glob'")"
  done

  # Each required script line deleted in turn.
  for line in "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}"; do
    expect_block "required script line '$line' deleted fires" "missing-script $line" \
      "$(printf '%s' "$wired" | grep -vxF -e "      $line")"
  done

  # ORDER, not merely presence: ci_targets.py compares a SET of stripped lines, so moving
  # `set -euo pipefail` below the invocations leaves every registry entry green while errexit
  # stops mattering. 8e is already reading the block in order, so it closes that here.
  expect_block 'set -euo pipefail moved last fires out-of-order' \
    'out-of-order-script set -euo pipefail' \
'tasks:
  affected-smoke:
    script: |
      ci/affected-graph/run.sh --negative-control
      ci/affected-graph/run.sh
      set -euo pipefail
    inputs:
'"$(for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do printf "      - '%s'\n" "$glob"; done)"'
'

  # A commented-out required line must report MISSING. This is the property whole-line matching
  # buys: commenting a line out does not remove its text, only prefix it.
  expect_block 'a commented-out script line still fires' \
    'missing-script ci/affected-graph/run.sh' \
    "$(printf '%s' "$wired" | sed "s|^      ci/affected-graph/run.sh\$|      # ci/affected-graph/run.sh|")"

  # SEPARATE STREAMS: a required INPUT string planted in the script body must not satisfy the
  # inputs table. ci_targets.py:842-843 documents why a concatenated haystack is wrong.
  expect_block 'an input string in the script body does not satisfy the inputs table' \
    'missing-input CLAUDE.md' \
    "$(printf '%s' "$wired" | grep -vxF -e "      - 'CLAUDE.md'" \
       | sed "s|^      set -euo pipefail\$|      set -euo pipefail\n      # CLAUDE.md|")"

  # Quote styles: moon accepts all three, so all three must be recognised.
  expect_block 'a double-quoted input is recognised' '' \
    "$(printf '%s' "$wired" | sed "s|^      - 'CLAUDE.md'\$|      - \"CLAUDE.md\"|")"
  expect_block 'an unquoted input is recognised' '' \
    "$(printf '%s' "$wired" | sed "s|^      - 'CLAUDE.md'\$|      - CLAUDE.md|")"
  expect_block 'a trailing comment on an unquoted input is stripped' '' \
    "$(printf '%s' "$wired" | sed "s|^      - 'CLAUDE.md'\$|      - CLAUDE.md  # the docs pin|")"

  # An interleaved YAML comment inside the sequence is not an entry. The live file has these
  # at moon.yml:181-202, so "any six-space line" is not a sufficient rule.
  expect_block 'an interleaved comment in the inputs block is skipped' '' \
    "$(printf '%s' "$wired" | sed "s|^      - 'CLAUDE.md'\$|      # SMA-541 — do not remove\n      - 'CLAUDE.md'|")"

  # Shapes this extractor refuses to guess at. Each reports its OWN token and nothing else:
  # a block we could not parse cannot support a per-line answer, and 19 missing-input rows on
  # top would bury the real problem.
  expect_block 'a folded script scalar fires bad-script-form' 'bad-script-form' \
    "$(printf '%s' "$wired" | sed 's|^    script: |$|    script: >|')"
  expect_block 'an inline inputs sequence fires bad-inputs-form' 'bad-inputs-form' \
'tasks:
  affected-smoke:
    script: |
      set -euo pipefail
    inputs: [moon.yml]
'
  expect_block 'a non-comment tail on the task key fires bad-task-form' 'bad-task-form' \
    "$(printf '%s' "$wired" | sed 's|^  affected-smoke:$|  affected-smoke: \&anchor|')"
  expect_block 'a trailing comment on the task key is tolerated' '' \
    "$(printf '%s' "$wired" | sed 's|^  affected-smoke:$|  affected-smoke:  # the cascade gate|')"
  expect_block 'a second inputs key fires duplicate-key' 'duplicate-key inputs' \
    "$wired    inputs:
      - 'moon.yml'
"
  expect_block 'the task being absent entirely fires no-task' 'no-task' \
'tasks:
  other-task:
    script: '"'"'true'"'"'
'
  # A LATER task must not be read as part of this one: the two-space key rule is what stops it.
  expect_block 'a required input declared on a DIFFERENT task does not count' \
    'missing-input CLAUDE.md' \
    "$(printf '%s' "$wired" | grep -vxF -e "      - 'CLAUDE.md'")"'    inputs:
      - '"'"'CLAUDE.md'"'"'
'

  # REQUIRED_INPUT_SKIP, both directions.
  saved_skip=(${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"})
  REQUIRED_INPUT_SKIP=("CLAUDE.md # moved to a different gate, verified by X")
  expect_block 'a skipped required input is not reported' '' \
    "$(printf '%s' "$wired" | grep -vxF -e "      - 'CLAUDE.md'")"
  expect_block 'a skip does not leak to a different glob' 'missing-input moon.yml' \
    "$(printf '%s' "$wired" | grep -vxF -e "      - 'moon.yml'")"
  REQUIRED_INPUT_SKIP=("CLAUDE.md")
  expect_block 'a skip with no reason is rejected' \
    'skip-without-reason CLAUDE.md
missing-input CLAUDE.md' \
    "$(printf '%s' "$wired" | grep -vxF -e "      - 'CLAUDE.md'")"
  REQUIRED_INPUT_SKIP=("ops/**/* # names a glob that is not required")
  expect_block 'a skip naming a non-required glob is reported stale' \
    'stale-skip ops/**/*' "$wired"
  REQUIRED_INPUT_SKIP=(${saved_skip+"${saved_skip[@]}"})

  # File-level verdicts, mirroring affected_graph_wiring_self_test's own two rows. The directory
  # case is portable because the guard is a bash builtin `test`, reached before any grep — there
  # is no BSD/GNU grep-on-a-directory split to disagree across platforms here.
  got="$(affected_smoke_block_verdict /nonexistent/moon.yml)"
  if [ "$got" != 'no-file' ]; then
    fail "affected-smoke-block self-test 'missing file': got '$got', expected 'no-file'. A
      renamed moon.yml must not be misread as every required input deleted."
    rc=1
  fi
  local unreadable_dir
  unreadable_dir="$(mktemp -d)"
  got="$(affected_smoke_block_verdict "$unreadable_dir")"
  rmdir "$unreadable_dir"
  if [ "$got" != 'no-file' ]; then
    fail "affected-smoke-block self-test 'directory in place of file': got '$got', expected
      'no-file'."
    rc=1
  fi

  return $rc
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
ci/actionlint/run.sh --self-test
```

Expected: FAIL. The table is defined but not called, so check 7's definition-count assertion
fires first: `self-test definitions: 10 '*_self_test' functions are defined but SELF_TEST_COUNT
is 9`. That is the correct first failure — it proves the counter notices an unwired table.

- [ ] **Step 3: Add the tables, the skip registry and the extractor**

Insert after `affected_graph_wiring_verdict`'s closing brace (~line 2065).

```bash
# ---------------------------------------------------------------------------------------------
# T_AFFECTED_SMOKE_* / affected_smoke_block_verdict — Check 8e (SMA-572 / SMA-573).
#
# Every pin in ci/affected-graph/ci_targets.py — RUN_SH_CALL_SITES, SELF_SCHEDULED_GATES,
# ACTIONLINT_SH_CALL_SITES, RELEASE_PARITY_SH_CALL_SITES — fires only when repo:affected-smoke is
# SCHEDULED, and until this check nothing pinned the `inputs` list that schedules it. Removing
# `- 'moon.yml'` is self-concealing: the removal is itself a root-moon.yml edit, and afterwards
# the task's remaining globs do not match that file (`*/moon.yml` matches rs/moon.yml, not
# moon.yml; `.moon/**/*` does not match it either), so the removal PR does not schedule the gate
# and every later PR can delete a pinned line with nothing red. MEASURED at moon 2.3.2: the root
# moon.yml is NOT an implicit input to the repo project's own tasks (repo:deny resolves to
# inputFiles ['rs/Cargo.lock','rs/deny.toml'] and no moon.yml), so there is no fallback.
#
# repo:input-liveness does not close this: it asserts a DECLARED glob still matches a tracked
# file, not that a required glob is still DECLARED.
#
# This lives HERE, not in ci_targets.py, because that file runs INSIDE repo:affected-smoke and
# would be the sole judge of its own reachability. repo:actionlint's inputs are ['**/*'] —
# MEASURED to match dot-prefixed paths, so a .github/-only PR does schedule it — and that
# premise is itself now pinned, from ci_targets.py's SELF_TASK_EXPECTED_GLOBS["actionlint"].
#
# CONTAINMENT, not equality: the list is nineteen entries and legitimately grows every time a
# gate keys on a new directory, so an exact match would red on every honest addition. The set
# below is the WHOLE current list rather than a judged subset — a floor, not a judgement call.
# The first design draft picked seven by a stated principle and an adversarial review showed the
# principle pulls in most of the rest anyway: cargo_moon_parity.py:478-479 reads every crate
# Cargo.toml from disk (so rs/**/Cargo.toml qualifies), and a crate's own moon.yml is not an
# input to its own tasks (cargo_moon_parity.py:315, SMA-528 F5), which is exactly WHY this gate
# must key on the four */moon.yml families — drop rs/crates/*/*/moon.yml and a PR changing only a
# crate's dependsOn or fileGroups.upstreams serves a cached PASS on the very edit A5/A6 exist to
# catch. Making it the whole list removes the "is this one load-bearing?" question the next
# reviewer would otherwise have to re-litigate.
T_AFFECTED_SMOKE_REQUIRED_INPUTS=(
  'ci/affected-graph/**/*'
  '.github/workflows/ci.yml'
  '.moon/**/*'
  'moon.yml'
  '*/moon.yml'
  'rs/crates/*/*/moon.yml'
  'py/packages/*/moon.yml'
  'ts/packages/*/moon.yml'
  'ts/apps/*/moon.yml'
  'rs/**/Cargo.toml'
  'py/packages/*/pyproject.toml'
  'rs/crates/*/*/pyproject.toml'
  'rs/crates/*/*/package.json'
  'ts/packages/*/package.json'
  'ts/apps/*/package.json'
  'ci/actionlint/**/*'
  'ci/release-parity/**/*'
  'CLAUDE.md'
  '.prototools'
)

# The same three lines ci_targets.py's SELF_SCHEDULED_GATES["affected-smoke"] pins — but pinned
# from here as well, and IN ORDER, for two reasons that copy cannot cover:
#   1. run.sh:405-409 exits inside the --negative-control branch, before run_suite at :412, so
#      deleting the bare `ci/affected-graph/run.sh` line leaves only the control, which asserts
#      against synthetic fixtures and exits 0. ci_targets.py never runs, so its own pin on that
#      line has no true-positive coverage at all. This check is scheduled independently, so it
#      survives exactly that deletion.
#   2. check_self_invocation compares a SET of stripped lines (ci_targets.py:851), so moving
#      `set -euo pipefail` below the invocations keeps every registry entry green while Moon —
#      which takes a script block's status from its LAST command — silently stops propagating a
#      failing control. Reading the block in order costs nothing here and closes that.
T_AFFECTED_SMOKE_REQUIRED_SCRIPT=(
  'set -euo pipefail'
  'ci/affected-graph/run.sh --negative-control'
  'ci/affected-graph/run.sh'
)

# The escape hatch, mirroring T_EXEMPT / ALLOW_DEAD_INPUT / BRANCH_SKIP / COE_SKIP: a required
# input can only be legitimately removed with a stated reason, so the edit is reviewable rather
# than indistinguishable from an attacker's. Entries are "<glob> # <why, and what covers it
# instead>". An entry naming a glob that is still declared is reported as stale, so a skip
# cannot outlive its glob.
#
# This is also the resolution of the one conflict with repo:input-liveness: if a directory a
# required glob names is ever RENAMED, task_inputs.py demands the dead glob be removed while this
# check demands it stay. Update T_AFFECTED_SMOKE_REQUIRED_INPUTS in the same commit —
# ALLOW_DEAD_INPUT is NOT an escape from this check.
REQUIRED_INPUT_SKIP=(
  # (empty — add entries as "<glob> # why, and what verifies it instead")
)

# rc 0 if $1 is skip-listed with a non-empty reason; rc 2 if listed with no reason (the caller
# reports it and still requires the glob); rc 1 otherwise.
is_required_input_skipped() {
  local key="$1" s glob reason
  for s in ${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"}; do :; done
  for s in ${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"}; do
    glob="${s%%#*}"; glob="${glob%"${glob##*[![:space:]]}"}"
    [ "$glob" = "$key" ] || continue
    reason="${s#*#}"
    case "$s" in *'#'*) ;; *) return 2 ;; esac
    [ -n "${reason//[[:space:]]/}" ] || return 2
    return 0
  done
  return 1
}

# Emits TAB-separated records for the repo:affected-smoke task block, in FILE ORDER:
#   INPUT\t<glob>    one `inputs:` sequence entry, surrounding quotes and any trailing comment
#                    on an UNQUOTED value stripped
#   SCRIPT\t<line>   one line of the `script: |` literal block, dedented by six
#   ERR\t<token>     a shape this extractor refuses to guess at
#
# Hand-rolled YAML, held to moon.yml's actual block style: `tasks:` at column 0, task keys at two
# spaces, field keys at four, sequence entries and script body at six. Anything else emits an ERR
# token rather than falling through in silence — the same rule CLAUDE.md already records for this
# file's workflow-filter extractor, and for the same reason: a parser that skips quietly turns
# the check it feeds into a vacuous pass.
#
# THIS IS NOT REACHABILITY ANALYSIS. Like checks 8/8b/8c/8d it matches lines; a required line
# parked in a never-executed block still satisfies it. See README Limitations.
affected_smoke_block_extract() {
  awk '
    function err(tok) { print "ERR\t" tok }

    # A task key sits at EXACTLY two spaces. Matching every such line — not just this task’s —
    # is what closes the block when the NEXT task starts; without it a required input declared
    # on a later task would satisfy this one.
    /^  [^ \t#][^:]*:/ {
      intask = 0; inscript = 0; ininputs = 0
      key = $0; sub(/^  /, "", key); sub(/:.*$/, "", key)
      if (key != "affected-smoke") next
      seen_task = 1
      tail = $0; sub(/^  [^:]*:[ \t]*/, "", tail); sub(/[ \t]*#.*$/, "", tail)
      if (tail != "") { err("bad-task-form"); next }
      intask = 1; task_ok = 1
      next
    }

    !intask { next }

    /^    script:/ {
      inscript = 0; ininputs = 0
      v = $0; sub(/^    script:[ \t]*/, "", v); sub(/[ \t]*#.*$/, "", v)
      if (seen_script) { err("duplicate-key script"); next }
      seen_script = 1
      if (v != "|") { err("bad-script-form"); next }
      inscript = 1; next
    }

    /^    inputs:/ {
      inscript = 0; ininputs = 0
      v = $0; sub(/^    inputs:[ \t]*/, "", v); sub(/[ \t]*#.*$/, "", v)
      if (seen_inputs) { err("duplicate-key inputs"); next }
      seen_inputs = 1
      if (v != "") { err("bad-inputs-form"); next }
      ininputs = 1; next
    }

    # Any other four-space field key closes whichever block was open.
    /^    [^ \t]/ { inscript = 0; ininputs = 0; next }

    inscript {
      if ($0 ~ /^[ \t]*$/) next                  # a blank line is literal-block content
      if ($0 ~ /^      /) { s = $0; sub(/^      /, "", s); print "SCRIPT\t" s; next }
      inscript = 0; next
    }

    ininputs {
      if ($0 ~ /^[ \t]*$/) next
      if ($0 ~ /^      #/) next                  # an interleaved YAML comment, not an entry
      if ($0 ~ /^      -[ \t]/) {
        v = $0; sub(/^      -[ \t]*/, "", v)
        # A trailing comment is stripped only on an UNQUOTED value: a quoted glob may legitimately
        # contain a `#`, and moon would read it as part of the pattern. \047 is a single quote —
        # spelled numerically so this awk program can stay inside single quotes.
        if (v ~ /^\047/)  { sub(/^\047/, "", v); sub(/\047[ \t]*(#.*)?$/, "", v) }
        else if (v ~ /^"/) { sub(/^"/, "", v); sub(/"[ \t]*(#.*)?$/, "", v) }
        else               { sub(/[ \t]*#.*$/, "", v); sub(/[ \t]+$/, "", v) }
        print "INPUT\t" v; next
      }
      ininputs = 0; next
    }

    END {
      if (!seen_task) { print "ERR\tno-task"; exit }
      if (!task_ok) exit
      if (!seen_script) print "ERR\tbad-script-form"
      if (!seen_inputs) print "ERR\tbad-inputs-form"
    }
  ' "$1"
}

# Echoes one verdict token per problem, and nothing for a wired block:
#   no-file                        moon.yml missing, or not a readable regular file
#   no-task | bad-task-form | bad-script-form | bad-inputs-form | duplicate-key <name>
#                                  the block could not be parsed — see the extractor's contract
#   missing-input <glob>           a required input is no longer declared
#   missing-script <line>          a required script line is absent (a COMMENTED-OUT copy counts
#                                  as absent — that is what whole-line matching buys)
#   out-of-order-script <line>     present, but after a line that must follow it
#   skip-without-reason <glob>     a REQUIRED_INPUT_SKIP entry with no stated reason
#   stale-skip <glob>              a REQUIRED_INPUT_SKIP entry naming a non-required glob
#
# Never `infra` from inside this function: it is invoked at the production call site as
# `done < <(affected_smoke_block_verdict ...)`, so it runs inside that process substitution's OWN
# subshell — an `exit 2` would exit only the subshell, FAILED would never be set, and the gate
# would finish rc 0 having asserted nothing. Echo a token and `return`, always. (Same bug
# CodeRabbit found on invocation_allowlist_verdict; see the comment above that function.)
affected_smoke_block_verdict() {
  local f="$1" recs glob line s idx prev=0 script_lines

  [ -f "$f" ] && [ -r "$f" ] || { echo 'no-file'; return; }

  recs="$(affected_smoke_block_extract "$f")"

  # A block we could not parse cannot support a per-line answer, and nineteen missing-input rows
  # on top of the real problem would bury it. Report the structural verdict alone.
  if printf '%s\n' "$recs" | grep -q "^ERR$(printf '\t')"; then
    printf '%s\n' "$recs" | sed -n "s/^ERR$(printf '\t')//p"
    return
  fi

  # Stale and reasonless skips are reported before the requirements they claim to waive, so a
  # typo'd entry cannot silently un-require a glob.
  for s in ${REQUIRED_INPUT_SKIP+"${REQUIRED_INPUT_SKIP[@]}"}; do
    glob="${s%%#*}"; glob="${glob%"${glob##*[![:space:]]}"}"
    [ -n "$glob" ] || continue
    printf '%s\n' "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" | grep -qxF -e "$glob" \
      || echo "stale-skip $glob"
  done

  for glob in "${T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}"; do
    is_required_input_skipped "$glob"
    case $? in
      0) continue ;;
      2) echo "skip-without-reason $glob" ;;
    esac
    printf '%s\n' "$recs" | grep -qxF -e "INPUT$(printf '\t')$glob" \
      || echo "missing-input $glob"
  done

  script_lines="$(printf '%s\n' "$recs" | sed -n "s/^SCRIPT$(printf '\t')//p")"
  for line in "${T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}"; do
    idx="$(printf '%s\n' "$script_lines" | grep -nxF -e "$line" | head -1 | cut -d: -f1)"
    if [ -z "$idx" ]; then
      echo "missing-script $line"
    elif [ "$idx" -le "$prev" ]; then
      echo "out-of-order-script $line"
    else
      prev="$idx"
    fi
  done
}
```

- [ ] **Step 4: Wire the fixture table in and bump the counter**

At `ci/actionlint/run.sh:40-41`, change the count and extend the inline name list:

```bash
SELF_TEST_COUNT=10  # extractor, path-filter, branch-filter, config, ci-target-floor,
                    # invocation-allowlist, affected-graph-wiring, block-execution,
                    # kill-predicate, affected-smoke-block
```

In `run_self_tests` (~line 3446), add the call after `kill_predicate_self_test`:

```bash
  affected_smoke_block_self_test
```

In `usage()` (~line 56), change "the nine fixture tables" to "the ten fixture tables" and append
`affected-smoke block` to the list.

- [ ] **Step 5: Run the self-test to verify it passes**

```bash
ci/actionlint/run.sh --self-test
```

Expected: exits 0. Check 9's mutation battery is deliberately not part of `--self-test`, so this
is the fast loop while iterating on the awk.

- [ ] **Step 6: Prove the new table is actually counted**

The battery derives its mutants from `run_self_tests`' own body, so the tenth table must extend
it. Verify by deleting the call and confirming the counter — not merely "something failed" —
is what reds.

```bash
sed -i.tmp '/^  affected_smoke_block_self_test$/d' ci/actionlint/run.sh && rm -f ci/actionlint/run.sh.tmp
ci/actionlint/run.sh --self-test; echo "rc=$?"
```

Expected: non-zero, naming `10 '*_self_test' functions are defined but SELF_TEST_COUNT is 10`
— specifically the invocation-count message from check 9's precondition or check 7's counter.
Then restore with `git checkout -- ci/actionlint/run.sh` **only if** no other edits are pending;
otherwise re-add the line by hand.

- [ ] **Step 7: Run the full gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
time ci/actionlint/run.sh
```

Expected: exits 0. Record the wall-clock; the budget is baseline (34.6s) + 10%.

- [ ] **Step 8: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "ci: add check 8e's moon.yml extractor and fixture table (SMA-572)"
```

---

## Task 3: Apply check 8e, and pin it back from `ci_targets.py`

Implements spec §3's arity floors and §4's mutual guard. Deliverable: 8e runs against the real
`moon.yml`, and its call site plus its table's arity floor are pinned from the other gate.

**Files:**
- Modify: `ci/actionlint/run.sh` — new production block after check 8d's
  `done < <(block_execution_verdict .github/workflows/ci.yml)` (~line 3821)
- Modify: `ci/affected-graph/ci_targets.py` — `ACTIONLINT_SH_CALL_SITES` (~418) and
  `self_test()`'s `wired_actionlint` fixture (~1348)

**Interfaces:**
- Consumes: `affected_smoke_block_verdict` and `T_AFFECTED_SMOKE_REQUIRED_INPUTS` from Task 2;
  the registries from Task 1.
- Produces: two column-0 lines in `ci/actionlint/run.sh` matched verbatim by
  `ACTIONLINT_SH_CALL_SITES`.

- [ ] **Step 1: Write the failing check — prove an emptied table is currently silent**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import re
src = open("ci/actionlint/run.sh").read()
mutated = re.sub(r"T_AFFECTED_SMOKE_REQUIRED_INPUTS=\(.*?\n\)\n",
                 "T_AFFECTED_SMOKE_REQUIRED_INPUTS=()\n", src, flags=re.S)
assert mutated != src, "the array literal did not match — check the pattern"
open("/tmp/mutated_run.sh", "w").write(mutated)
PY
chmod +x /tmp/mutated_run.sh
bash /tmp/mutated_run.sh --self-test; echo "rc=$?"
```

Expected: **rc=0** with the table emptied — the verdict function iterates the array, so an empty
one emits zero verdicts and every fixture row passes for the wrong reason. That is the defect
the arity floor closes.

- [ ] **Step 2: Add the arity floors and the production call site**

Append after check 8d's block (~line 3821, inside the same `if` that guards checks 8-8d if one
is present — match the surrounding structure exactly).

```bash
# ---------------------------------------------------------------------------------------------
# Check 8e — repo:affected-smoke still declares the inputs that schedule every pin in
# ci/affected-graph/ci_targets.py, and still runs both halves of its own script, in order.
# Rationale, tables and fixtures are with affected_smoke_block_verdict above.
#
# THE ARITY FLOORS ARE PART OF THE CHECK, not a sanity nicety. affected_smoke_block_verdict
# iterates its tables, so an EMPTIED table emits zero verdicts and passes — the "green while
# asserting nothing" failure this whole registry exists to prevent. Check 8c is immune to that
# only because its table is a verbatim dual copy of ci_targets.py's RUN_SH_CALL_SITES and the
# other copy still asserts the same lines; 8e keeps its set at ONE site (a second copy would add
# drift risk with no added coverage, since repo:actionlint runs on every PR), so it buys
# non-emptiability this way instead. Both floors are pinned from ci_targets.py's
# ACTIONLINT_SH_CALL_SITES, which makes shrinking either table a two-file edit across two
# independently scheduled gates. `-ge`, not `-eq`, so honest GROWTH needs no second edit.
# ---------------------------------------------------------------------------------------------
[ "${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" -ge 19 ] || infra "check 8e: T_AFFECTED_SMOKE_REQUIRED_INPUTS has ${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]} entries, expected at least 19"
[ "${#T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]}" -ge 3 ] || infra "check 8e: T_AFFECTED_SMOKE_REQUIRED_SCRIPT has ${#T_AFFECTED_SMOKE_REQUIRED_SCRIPT[@]} entries, expected at least 3"

while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail "moon.yml does not exist, or is not a readable regular file, so this gate cannot
      confirm repo:affected-smoke still declares the inputs that schedule every pin in
      ci/affected-graph/ci_targets.py. If it was renamed, update check 8e in $0." ;;
    no-task)
      fail "moon.yml no longer declares a task named 'affected-smoke'. That task is where
      ci_targets.py runs, so removing it switches off C1-C5, the cascade cases, A1-A6 and every
      call-site pin in that file at once." ;;
    bad-task-form|bad-script-form|bad-inputs-form)
      fail "check 8e could not parse repo:affected-smoke's block in moon.yml ($verdict). The
      extractor is held to this file's block style — task keys at two spaces, fields at four,
      'script: |' as a literal block and 'inputs:' as a block sequence. It fails loudly rather
      than skipping in silence, because a parser that skips quietly makes this check vacuous.
      Restore the block style, or teach affected_smoke_block_extract the new one (with a
      fixture)." ;;
    'duplicate-key '*)
      fail "repo:affected-smoke declares '${verdict#duplicate-key }' twice in moon.yml. Which one
      moon honours is not something this check will guess at — merge them." ;;
    'missing-input '*)
      fail "repo:affected-smoke no longer declares the input '${verdict#missing-input }'. Every
      pin in ci/affected-graph/ci_targets.py fires only when this task is SCHEDULED, and its
      inputs are what schedule it — so dropping one silently un-reaches a whole family of pins
      for every later PR. The 'moon.yml' entry is the worst case: removing it is itself a
      root-moon.yml edit, and afterwards nothing in this task's remaining globs matches that
      file, so the removal PR is the last one that would have noticed. Restore the entry, or add
      a reasoned REQUIRED_INPUT_SKIP entry in $0 saying what covers it instead." ;;
    'missing-script '*)
      fail "repo:affected-smoke's script no longer contains the exact line
      '${verdict#missing-script }' (a commented-out copy counts as absent). Moon takes a script
      block's status from its LAST command, so all three lines are load-bearing: without
      'set -euo pipefail' a failing negative control is swallowed, and without the bare
      'ci/affected-graph/run.sh' only the control runs — which asserts against synthetic
      fixtures and exits 0. ci_targets.py cannot catch that last one: it is what would have run
      the checker. This check is scheduled independently, so it survives the deletion." ;;
    'out-of-order-script '*)
      fail "repo:affected-smoke's script line '${verdict#out-of-order-script }' appears AFTER a
      line that must follow it. ci_targets.py compares a set of lines and cannot see order, so
      moving 'set -euo pipefail' below the invocations leaves it green while errexit stops
      applying to them. Restore the documented order." ;;
    'skip-without-reason '*)
      fail "REQUIRED_INPUT_SKIP entry for '${verdict#skip-without-reason }' states no reason.
      Write it as \"<glob> # why, and what verifies it instead\" — an unexplained waiver is
      indistinguishable from the deletion this check exists to catch." ;;
    'stale-skip '*)
      fail "REQUIRED_INPUT_SKIP names '${verdict#stale-skip }', which is not in
      T_AFFECTED_SMOKE_REQUIRED_INPUTS. A waiver that has outlived its requirement hides the next
      one; delete it." ;;
    *)
      infra "unhandled affected-smoke-block verdict '$verdict'" ;;
  esac
done < <(affected_smoke_block_verdict moon.yml)
```

- [ ] **Step 3: Run the gate to verify it passes on the real tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh; echo "rc=$?"
```

Expected: rc=0. A `missing-input` row here means `T_AFFECTED_SMOKE_REQUIRED_INPUTS` and
`moon.yml` disagree — reconcile against Task 1 Step 1's live output, not against this plan.

- [ ] **Step 4: Re-run Step 1's mutation — it must now fire**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import re
src = open("ci/actionlint/run.sh").read()
mutated = re.sub(r"T_AFFECTED_SMOKE_REQUIRED_INPUTS=\(.*?\n\)\n",
                 "T_AFFECTED_SMOKE_REQUIRED_INPUTS=()\n", src, flags=re.S)
open("/tmp/mutated_run.sh", "w").write(mutated)
PY
bash /tmp/mutated_run.sh; echo "rc=$?"
```

Expected: **rc=2** (an `infra` exit) naming
`T_AFFECTED_SMOKE_REQUIRED_INPUTS has 0 entries, expected at least 19`.

- [ ] **Step 5: Add both lines to `ACTIONLINT_SH_CALL_SITES`**

In `ci/affected-graph/ci_targets.py`, append to the tuple (~line 418):

```python
    # Check 8e's production call site (SMA-572/SMA-573) — the pin that makes
    # SELF_TASK_GLOBS_EXEMPT["affected-smoke"] a delegation rather than a skip. Same shape as
    # the four entries above it: `affected_smoke_block_verdict` is also called from inside its
    # own self-test fixtures (`affected_smoke_block_verdict "$tmp"`,
    # `affected_smoke_block_verdict /nonexistent/moon.yml`), so a substring test would be
    # satisfied by those and survive deleting this exact production line.
    "done < <(affected_smoke_block_verdict moon.yml)",
    # ...and the INPUT table's arity floor, which is a second, different hole. The verdict
    # function iterates its table, so an EMPTIED table emits zero verdicts and the gate passes
    # having asserted nothing — measured before this floor existed: `run.sh --self-test` with
    # the array replaced by `()` exited 0 with every fixture row green. Check 8c never needed
    # this because its table is a verbatim dual copy of RUN_SH_CALL_SITES above and the other
    # copy still asserts the lines; 8e deliberately keeps its set at one site, so the floor is
    # what makes shrinking it a two-file edit across two independently scheduled gates.
    '[ "${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" -ge 19 ] || infra "check 8e: T_AFFECTED_SMOKE_REQUIRED_INPUTS has ${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]} entries, expected at least 19"',
```

- [ ] **Step 6: Extend the `wired_actionlint` fixture**

`self_test()`'s positive control asserts `check_self_invocation` fires on nothing when all four
haystacks are wired, so the fixture must carry the two new lines or that control reds. In
`ci_targets.py` (~line 1348), append inside the `wired_actionlint` string, before its closing
paren:

```python
        # Check 8e's production call site and its input-table arity floor (SMA-572) — both
        # whole-line matched at column 0, for the same reason as the four entries above: each
        # appears in substring form elsewhere (the verdict function inside its own fixtures; the
        # array name in its own declaration), so only the whole line proves the PRODUCTION use.
        'done < <(affected_smoke_block_verdict moon.yml)\n'
        '[ "${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]}" -ge 19 ] || infra "check 8e: T_AFFECTED_SMOKE_REQUIRED_INPUTS has ${#T_AFFECTED_SMOKE_REQUIRED_INPUTS[@]} entries, expected at least 19"\n'
```

- [ ] **Step 7: Run both gates' self-tests**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test; echo "ci_targets rc=$?"
ci/actionlint/run.sh --self-test; echo "actionlint rc=$?"
```

Expected: both rc=0.

- [ ] **Step 8: Prove the two new pins bite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
for pat in 'done < <(affected_smoke_block_verdict moon.yml)' '-ge 19 ] || infra'; do
  grep -vF -- "$pat" ci/actionlint/run.sh > /tmp/mut.sh
  python3 - "$pat" <<'PY'
import sys, json, subprocess
sys.path.insert(0, "ci/affected-graph")
import ci_targets
out = subprocess.run(["moon","query","tasks"], capture_output=True, text=True, check=True).stdout
scripts = ci_targets._scripts(json.loads(out)["tasks"])
missing = ci_targets.check_self_invocation(
    open("ci/affected-graph/run.sh").read(), scripts,
    open("/tmp/mut.sh").read(), open("ci/release-parity/run.sh").read())
print(sys.argv[1], "->", [m for m in missing if "actionlint" in m])
PY
done
```

Expected: each iteration prints a non-empty list naming the deleted line.

- [ ] **Step 9: Run both gates end to end**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control && ci/affected-graph/run.sh && ci/actionlint/run.sh
echo "rc=$?"
```

Expected: rc=0.

- [ ] **Step 10: Commit**

```bash
git add ci/actionlint/run.sh ci/affected-graph/ci_targets.py
git commit -m "ci: apply check 8e and pin its call site and arity floor (SMA-572)"
```

---

## Task 4: Verification sweep

Implements spec §5. Deliverable: a recorded evidence block for the PR body. This task changes no
production code — every mutation is reverted.

**Files:**
- Create: `/tmp/sma-572-evidence.md` (scratch; its contents go in the PR body, not the repo)
- Mutates and restores: `moon.yml`, `ci/actionlint/run.sh`, `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: everything from Tasks 1-3.
- Produces: the evidence block. No code.

- [ ] **Step 1: Positive controls**

A table whose rows all fire cannot tell a working check from a stuck one.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git status --porcelain          # must be clean before starting
bash -c 'source /dev/stdin <<< "$(sed -n "/^affected_smoke_block_verdict()/,/^}/p" ci/actionlint/run.sh)"' 2>/dev/null || true
ci/actionlint/run.sh; echo "unmutated actionlint rc=$?"
python3 ci/affected-graph/ci_targets.py --self-test; echo "unmutated ci_targets rc=$?"
```

Expected: both rc=0, and the self-test's own `no-file`/`no-task`/directory rows (added in
Task 2) already ran inside it.

- [ ] **Step 2: The 10 `SELF_SCHEDULED_GATES` lines**

Delete each pinned line from `moon.yml` in turn and confirm `ci_targets.py` names it. Use the
Edit tool for each mutation and revert with `git checkout -- moon.yml` after each.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import json, subprocess, sys
sys.path.insert(0, "ci/affected-graph")
import ci_targets
out = subprocess.run(["moon","query","tasks"], capture_output=True, text=True, check=True).stdout
base = ci_targets._scripts(json.loads(out)["tasks"])
run_sh = open("ci/affected-graph/run.sh").read()
al = open("ci/actionlint/run.sh").read()
rp = open("ci/release-parity/run.sh").read()
ok = True
for task, lines in ci_targets.SELF_SCHEDULED_GATES.items():
    for line in lines:
        s = dict(base)
        s[task] = "".join(l + "\n" for l in s[task].splitlines() if l.strip() != line)
        got = ci_targets.check_self_invocation(run_sh, s, al, rp)
        hit = [m for m in got if m == f"{task} script: {line}"]
        print(("PASS" if hit else "FAIL"), task, "|", line)
        ok = ok and bool(hit)
print("all pinned lines bite:", ok)
PY
```

Expected: every row `PASS`, final line `True`. This drives the registry directly, which is
faithful because `check_self_invocation` is what CI runs; the `moon.yml` round-trip is covered
by Step 4's live case.

- [ ] **Step 3: The three exact `SELF_TASK_EXPECTED_GLOBS` entries**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
import json, subprocess, sys, copy
sys.path.insert(0, "ci/affected-graph")
import ci_targets
out = subprocess.run(["moon","query","tasks"], capture_output=True, text=True, check=True).stdout
tasks = json.loads(out)["tasks"]
for gate in ["actionlint", "publish-metadata", "error-code-single-site"]:
    t = copy.deepcopy(tasks)
    entry = t["repo"][gate]
    bucket = "inputGlobs" if entry.get("inputGlobs") else "inputFiles"
    victim = sorted(k for k in entry[bucket] if not k.startswith(".moon/*"))[0]
    del entry[bucket][victim]
    rows = ci_targets.check_gate_inputs(t)
    print(("PASS" if any(gate in r for r in rows) else "FAIL"), gate, "dropped", victim)
PY
```

Expected: three `PASS` rows. The `actionlint` row is the one that makes narrowing
`repo:actionlint` to `.github/workflows/**` red.

- [ ] **Step 4: The two self-concealing cases, three parts each**

These are the cases whose obvious test passes vacuously: invoking `ci/affected-graph/run.sh` by
hand after deleting it from `moon.yml` reds only because the hand invocation is the one path the
deletion does not break.

**4a — the real-run line.** Edit `moon.yml` to delete the bare `      ci/affected-graph/run.sh`
line from `affected-smoke`'s script, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force; echo "PART 1 (must be 0): $?"
ci/actionlint/run.sh; echo "PART 2 (must be non-zero): $?"
git checkout -- moon.yml
moon run repo:affected-smoke --force; echo "PART 3 (must be 0): $?"
```

Expected: part 1 exits **0** — proving Task 1's pin alone is blind; part 2 exits non-zero naming
`missing-script ci/affected-graph/run.sh`; part 3 exits 0.

**4b — the `moon.yml` input.** Edit `moon.yml` to delete `      - 'moon.yml'` from
`affected-smoke`'s `inputs:`, then run the same three parts. Expected: identical shape, with
part 2 naming `missing-input moon.yml`.

- [ ] **Step 5: 8e's 19 inputs, 3 script lines, order and emptied table**

The fixture table from Task 2 already drives every one of these under `--self-test`. Record that
it does, and spot-check the live path once:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh --self-test; echo "fixture table rc=$?"
grep -c "^      - '" moon.yml    # sanity: the live inputs count
```

- [ ] **Step 6: Cost measurement**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
for i in 1 2 3; do /usr/bin/time -f '%e' ci/actionlint/run.sh >/dev/null; done
```

(macOS has no GNU `time -f`; use `time ci/actionlint/run.sh > /dev/null` three times and take the
minimum `total`.) Baseline is **34.6s**; the budget is baseline + 10% = **38.1s**. Record the
min-of-3 and compare.

- [ ] **Step 7: The full graph, exactly as CI runs it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep --base origin/main \
  --include-relations
echo "rc=$?"
```

Expected: rc=0. If Moon reports an unattributed failure, diagnose with
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

- [ ] **Step 8: Confirm the tree is clean and commit the evidence into the PR body draft**

```bash
git status --porcelain          # must be empty — every mutation reverted
```

No commit for this task (it produces no repo changes). Carry `/tmp/sma-572-evidence.md` forward
to Task 5's PR body.

---

## Task 5: Documentation

Implements spec §6. Five sites carry counts or lists this change invalidates.

**Files:**
- Modify: `ci/affected-graph/README.md`, `ci/actionlint/README.md`,
  `ci/release-parity/README.md`, `CLAUDE.md`, `moon.yml` (comments only)

**Interfaces:**
- Consumes: the final shape of Tasks 1-3.
- Produces: nothing code depends on.

- [ ] **Step 1: `ci/affected-graph/README.md`**

In the C4 bullet (~line 120), extend the `SELF_SCHEDULED_GATES` clause to name the four new
gates and their line counts (three each for `affected-smoke`, `publish-metadata` and
`error-code-single-site`; one for `actionlint`), add check 8e to the `ACTIONLINT_SH_CALL_SITES`
clause, and in the pairing paragraph (~line 165) record that `publish-metadata`,
`error-code-single-site` and `actionlint` are exact-pinned while `affected-smoke` is delegated to
check 8e. State plainly that `affected-smoke`'s third pinned line has no true-positive coverage
from this file.

- [ ] **Step 2: `ci/actionlint/README.md`**

Add a `| 8e |` row to the checks table (~line 23) describing the two tables, the containment vs
whole-line-in-order distinction, the verdict vocabulary, the arity floors and
`REQUIRED_INPUT_SKIP`. Update the `| 7 |` and `| 9 |` rows from "nine" to "ten". In Limitations
(~line 109), add:

- **L13** — 8e matches lines, not reachability; a required line in an unindented never-executed
  block still satisfies it (same as L3's class).
- **L14** — `REQUIRED_INPUT_SKIP` is an unguarded escape hatch, as `COE_SKIP` and
  `SWALLOWED_SKIP` are; the defence is review, and the value is that the waiver is explicit.
- Update **L1** to note that deleting check 8e's block *and* both `ACTIONLINT_SH_CALL_SITES`
  entries in one edit is the same bounded two-gate shape.

- [ ] **Step 3: `ci/release-parity/README.md`**

L1 is closed by this change. Rewrite it to say so and point at check 8e, rather than leaving a
README asserting a hole that no longer exists.

- [ ] **Step 4: `CLAUDE.md`**

In the `release-parity*` gotcha, "SELF_SCHEDULED_GATES pins the nine `moon.yml` lines" now
undercounts. Rewrite that clause to state the registry's current membership without a bare
number that will rot again, e.g. "pins every self-scheduled gate's `moon.yml` invocation lines —
`set -euo pipefail` included — for `input-liveness`, the three `release-parity*`,
`version-lockstep`, `publish-metadata`, `error-code-single-site`, `affected-smoke` and
`actionlint`". Add one sentence recording that `repo:affected-smoke`'s own `inputs` and script
are pinned by check 8e in `ci/actionlint/run.sh`, and that `repo:actionlint`'s `['**/*']` is
pinned from `ci_targets.py`.

**Do not** touch the text between `<!-- ci-targets:begin -->` and `<!-- ci-targets:end -->`, and
do not introduce a second copy of either marker — a duplicate reds `repo:affected-smoke`.

- [ ] **Step 5: `moon.yml` comments**

The `actionlint:` task's comment block (~lines 594-620) hardcodes "nine fixture tables",
"`--self-test` ALONE … makes 9" and "ten concurrent subprocesses". Update all three to ten and
eleven respectively. These are comments only — do not change the task's `script:` or `inputs:`,
both of which are now pinned.

- [ ] **Step 6: Verify the docs edits did not break a gate**

`CLAUDE.md` and `moon.yml` are both inputs to `repo:affected-smoke`, and C3 compares CLAUDE.md's
marker-delimited command to `T` token for token.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh && ci/actionlint/run.sh; echo "rc=$?"
grep -c 'ci-targets:begin' CLAUDE.md   # must be exactly 1
grep -c 'ci-targets:end' CLAUDE.md     # must be exactly 1
```

Expected: rc=0 and both counts `1`.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/README.md ci/actionlint/README.md ci/release-parity/README.md CLAUDE.md moon.yml
git commit -m "docs(ci): record check 8e and the four new script pins (SMA-572)"
```

---

## Self-review

**Spec coverage.** §1 → Task 1 Step 3 (four entries, including the no-true-positive note).
§2 → Task 1 Step 4 (three exact pins, one exemption; the push-back about `publish-metadata`'s
control shape is documentation, folded into the registry comment). §3 → Task 2 (tables,
extractor contract shape-by-shape, `REQUIRED_INPUT_SKIP`, order assertion) and Task 3 Step 2
(arity floors). §4 → Task 3 Steps 5-6. §5 → Task 4, with the three-part demonstrations at
Step 4 and the cost budget at Step 6. §6 → Task 5, all five sites plus `ci/release-parity`'s L1.
Residuals R1/R2/R3/R5/R6 → Task 5 Step 2's L1/L13/L14 and the registry comments.

**Placeholder scan.** No TBDs. Every code step carries the actual text to insert. The one place
this plan defers to reality rather than to a literal is Task 1 Step 1, which is deliberate: the
tuples must match `moon query tasks`, and the plan says to prefer the live value over its own
transcription if they disagree.

**Type consistency.** `affected_smoke_block_extract` / `affected_smoke_block_verdict` /
`affected_smoke_block_self_test` / `is_required_input_skipped` /
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` / `T_AFFECTED_SMOKE_REQUIRED_SCRIPT` / `REQUIRED_INPUT_SKIP`
are spelled identically in Tasks 2, 3 and 4 and in the two `ACTIONLINT_SH_CALL_SITES` strings.
The arity-floor line appears three times — Task 3 Step 2 (the source), Step 5 (the pin) and
Step 6 (the fixture) — and must stay byte-identical in all three; Task 3 Step 8 is what proves it.
