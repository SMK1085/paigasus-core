<!-- moon-diagnosis:ok -->

# SMA-597 — Moon failure diagnosis procedure and its gate: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put a *demonstrated* diagnosis procedure for an unattributed `moon ci` failure into CLAUDE.md, annotate the 67 documents carrying the broken advice, and add check 12 to `repo:actionlint` so the corpus cannot grow.

**Architecture:** Three separable deliverables. (1) A marker-delimited CLAUDE.md block carrying the measured procedure. (2) A one-shot scripted annotation appending `<!-- moon-diagnosis:superseded -->` to the 67 historical documents — the advice text itself is left untouched. (3) Check 12 inside `ci/actionlint/run.sh`: a pure verdict pair driven by one fixture table, plus a production call site and two arity floors pinned from `ci/affected-graph/ci_targets.py`.

**Tech Stack:** bash (run.sh), Python (ci_targets.py), GitHub Actions YAML, Markdown.

**Spec:** `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md` (rev 3)

## Global Constraints

- **Bash 3.2 compatible.** macOS ships bash 3.2, which has no `declare -A`. Tables are indexed arrays of `path|reason` strings, never associative arrays.
- **Column 0, whole line.** Every line pinned by `ACTIONLINT_SH_CALL_SITES` must sit at column 0 — the pin matches with `rstrip` and no `lstrip`, deliberately, so an indented copy does not satisfy it.
- `run.sh` is `set -uo pipefail` with **no `-e`**. Every subprocess status must be explicitly routed, or the gate finishes rc 0 having asserted nothing.
- **`infra` (exit 2) vs `fail` (rc 1)**: a broken gate is `infra`; a wrong repository is `fail`. Never collapse them.
- **Exit codes:** `fail()` sets `FAILED=1` and returns; `infra()` exits 2 immediately.
- All shell added to `.github/workflows/**` must pass `repo:actionlint`'s shellcheck integration; all `ci/**/*.py` must pass `repo:ruff-ci`.
- SPDX header on every new source file: `# SPDX-License-Identifier: Apache-2.0`.
- Run every `moon` command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Commits: conventional, workspace-scoped, `(SMA-597)` in the subject. Do not use `--no-verify`.

## File Structure

| File | Responsibility |
| -- | -- |
| `CLAUDE.md` | the corrected procedure, marker-delimited; the `buffer-only-failure` correction; the self-test count prose |
| `ci/actionlint/run.sh` | check 12: two tables, two pure verdict functions, one self-test table, one production block with two arity floors |
| `ci/affected-graph/ci_targets.py` | three `ACTIONLINT_SH_CALL_SITES` entries pinning check 12's production lines |
| `.github/workflows/ci.yml` | `if: failure()` upload of the moon cache diagnostics |
| `ci/actionlint/README.md` | check 12's row, the re-counts, the "## Why" amendment, the new Limitations entries |
| `moon.yml` | the `repo:actionlint` comment block's "THIRTEEN"/"thirteen mutants" counts |
| 67 documents under `docs/superpowers/{plans,specs}/` | appended supersession marker, one scripted edit |

---

## Task 1: The corrected procedure in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (new gotcha bullet; correction to the existing `buffer-only-failure` sentence in the `affected-smoke` entry)

**Interfaces:**
- Consumes: nothing.
- Produces: the marker pair `<!-- moon-diagnosis:begin -->` / `<!-- moon-diagnosis:end -->` and the five literals Task 4's `DOC_DIAGNOSIS_REQUIRED_LITERALS` asserts: `operations[]`, `task-execution`, `.moon/cache/states/`, `stderr.log`, `lastRunTime`.

- [ ] **Step 1: Add the marker-delimited block to CLAUDE.md**

Insert as a new bullet in the Gotchas list, immediately after the `repo:affected-smoke` flake entry (it is the entry that most needs it). Exact content:

```markdown
- **Diagnosing an unattributed `moon ci` failure.** The procedure below is MEASURED on moon 2.5.3
  (SMA-597); re-take it on a bump. It is for **local** runs — in CI see the note at the end.
  <!-- moon-diagnosis:begin -->
  **Step 0 — capture before you re-run anything.** A re-run overwrites every artifact holding the
  evidence, and a PASSING re-run is just as destructive as another failing one: it rewrites
  `stdout.log`, truncates `stderr.log` to zero bytes, rewrites `lastRun.json`, and flips the
  action to `passed` in `ciReport.json` (all four measured). Copy `.moon/cache/ciReport.json` and
  `.moon/cache/states/<project>/<task>/` somewhere outside the repo first.

  **Step 1 — which task, what command, what exit code.**
  ```bash
  jq '.actions[] | select(.status=="failed")
      | {label, error,
         exec: (.operations[] | select(.meta.type=="task-execution") | {command, exitCode})}' \
     .moon/cache/ciReport.json
  ```
  There is **no action-level `exitCode` key** — `has("exitCode")` is `false`. The widely copied
  query projects `{label, status, exitCode}` and so reports `null` for a key moon never writes,
  which is why this file has a reputation for being empty. It is not: the real exit code and the
  full command are in `operations[]`, on the entry whose `meta.type` is `task-execution`.

  **Step 2 — why.** `cat .moon/cache/states/<project>/<task>/stdout.log` and `stderr.log`. This is
  the only place task output exists. It works for a task that never started: a missing binary
  leaves `stdout.log` empty and `stderr.log` holding `command not found` (exit 127).

  **Step 2a — prove the logs belong to this run. Mandatory.** Compare the report action's
  `finishedAt` against `lastRun.json`'s `lastRunTime`. **If they disagree, stop** — the logs are
  from a different run and pairing them with step 1's command yields a confident wrong answer.
  Two measured causes: `moon run` writes `runReport.json` and does NOT touch `ciReport.json`, so a
  `moon run` re-run desynchronises them; and a cache HIT rewrites neither, so a log can be
  arbitrarily older than the run you are looking at.

  **Step 3 — if it still does not reproduce.** `moon run <target> --force`. Note that
  `buffer-only-failure` prints a FAILING task's output but discards a PASSING one's, and that this
  re-run desynchronises the report and the logs per step 2a.

  **What cannot work:** no `--summary` level (`none`/`minimal`/`normal`/`detailed`) and no
  `outputStyle` (`stream`/`buffer`/`none`) puts stdout or stderr into `ciReport.json`; all seven
  cells are byte-identical, and a key walk over the whole failing action finds no output field at
  any depth. `--log-file` captures moon's own tracing, not a task's stdio. Do not re-litigate this.

  **In CI this procedure does not apply as written.** There is no shell and the runner is
  destroyed, so step 0 is unexecutable; and `ci.yml` restores `.moon/cache` across runs, so a
  cache-hit task's logs may be from an older commit entirely. Use the `moon-diagnostics` artifact
  that `ci.yml` uploads on failure.
  <!-- moon-diagnosis:end -->
```

- [ ] **Step 2: Correct the `buffer-only-failure` claim in the affected-smoke entry**

Find the existing sentence in the `repo:affected-smoke` gotcha that says a re-run destroys the evidence, and replace the mechanism. Current text ends `...because a re-run passes and destroys the evidence.` Append the correction:

```markdown
  (SMA-597 measured the mechanism: it is OVERWRITE, not discard. A passing re-run rewrites
  `stdout.log`, truncates `stderr.log`, rewrites `lastRun.json` and flips the `ciReport.json` row
  to `passed`. See the diagnosis procedure entry below.)
```

- [ ] **Step 3: Verify the markers are unique and ordered**

```bash
grep -c 'moon-diagnosis:begin' CLAUDE.md   # expect exactly 1
grep -c 'moon-diagnosis:end' CLAUDE.md     # expect exactly 1
grep -n 'moon-diagnosis:' CLAUDE.md        # begin must precede end
```

Expected: `1`, `1`, and begin on a lower line number than end.

- [ ] **Step 4: Verify all five required literals are inside the block**

```bash
awk '/moon-diagnosis:begin/,/moon-diagnosis:end/' CLAUDE.md > /tmp/block.txt
for lit in 'operations[]' 'task-execution' '.moon/cache/states/' 'stderr.log' 'lastRunTime'; do
  grep -qF -- "$lit" /tmp/block.txt && echo "OK   $lit" || echo "MISS $lit"
done
```

Expected: five `OK` lines. A `MISS` means Task 4's check 12 will red.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(repo): record the measured moon failure diagnosis procedure (SMA-597)"
```

---

## Task 2: Annotate the 67 historical documents

**Files:**
- Create: `/tmp/sma597-annotate.sh` (throwaway, not committed)
- Modify: 67 files under `docs/superpowers/plans/` and `docs/superpowers/specs/`
- Modify: `docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md` (add `:ok` marker)

**Interfaces:**
- Consumes: nothing.
- Produces: every historical document carries `<!-- moon-diagnosis:superseded -->`; the spec and this plan carry `<!-- moon-diagnosis:ok -->`. Task 5's production run depends on this being complete, or check 12 reds against the real tree.

- [ ] **Step 1: Record the baseline count**

```bash
git ls-files -z | xargs -0 grep -l 'ciReport' 2>/dev/null | sort > /tmp/sma597-before.txt
wc -l < /tmp/sma597-before.txt
```

Expected: `69` — the 67 historical documents, plus the spec and this plan (both already written).

- [ ] **Step 2: Write the annotation script**

```bash
cat > /tmp/sma597-annotate.sh <<'SCRIPT'
#!/usr/bin/env bash
# SMA-597 one-shot: append a supersession marker to each historical document carrying the
# broken ciReport advice. The advice text itself is NOT modified — these are dated records.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

NOTE='
<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.'

count=0
while IFS= read -r f; do
  case "$f" in
    docs/superpowers/plans/*|docs/superpowers/specs/*) ;;
    *) continue ;;
  esac
  # Skip the two SMA-597 documents — they get an :ok marker instead, in step 4.
  case "$f" in *sma-597*) continue ;; esac
  grep -q 'moon-diagnosis:superseded' "$f" && continue
  printf '%s\n' "$NOTE" >> "$f"
  count=$((count + 1))
done < /tmp/sma597-before.txt
echo "annotated $count files"
SCRIPT
chmod +x /tmp/sma597-annotate.sh
```

- [ ] **Step 3: Run it and verify the count**

```bash
/tmp/sma597-annotate.sh
```

Expected: `annotated 67 files`. If it says anything else, stop — the corpus is not what the spec measured, and §3.2's `-ge 60` floor assumes ~67.

- [ ] **Step 4: Add the `:ok` marker to the spec**

This plan already carries `<!-- moon-diagnosis:ok -->` on its first line. Add the same to the spec, as its first line, before the `#` heading:

```bash
printf '%s\n\n' '<!-- moon-diagnosis:ok -->' | cat - docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md > /tmp/spec.md
mv /tmp/spec.md docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md
head -1 docs/superpowers/specs/2026-09-03-sma-597-moon-failure-diagnosis-design.md
```

Expected: `<!-- moon-diagnosis:ok -->`

- [ ] **Step 5: Verify every token-carrying file is now covered**

```bash
git ls-files -z | xargs -0 grep -l 'ciReport' 2>/dev/null | while IFS= read -r f; do
  if ! grep -q 'moon-diagnosis:superseded\|moon-diagnosis:ok' "$f"; then echo "UNCOVERED $f"; fi
done
```

Expected: no output except `CLAUDE.md` — which is allowlisted in Task 4 rather than marked, since the authority does not self-certify. If `ci/actionlint/run.sh` or `ci/actionlint/README.md` appear, they do not exist yet; they arrive in Tasks 4 and 7 and are allowlisted.

- [ ] **Step 6: Confirm the advice text itself was not altered**

```bash
git diff --stat docs/superpowers/plans/ | tail -1
git diff docs/superpowers/plans/2026-08-19-sma-541-ci-target-coverage-gate.md
```

Expected: every file shows **only** additions at the end of file — no deletions, no modified lines. If any line was removed, revert and fix the script.

- [ ] **Step 7: Commit**

```bash
git add docs/superpowers/
git commit -m "docs(repo): mark the superseded ciReport diagnosis advice in place (SMA-597)"
```

---

## Task 3: Check 12's tables and verdict functions

**Files:**
- Modify: `ci/actionlint/run.sh` (add two tables and two verdict functions, before the `run_self_tests` block near line 4595)

**Interfaces:**
- Consumes: `fail()`, `infra()` from run.sh's header.
- Produces:
  - `CIREPORT_MENTIONS_ALLOWED` — indexed array of `path|reason` strings.
  - `DOC_DIAGNOSIS_REQUIRED_LITERALS` — indexed array of literal strings required inside CLAUDE.md's block.
  - `doc_diagnosis_verdict <paths-file>` — reads a newline-delimited list of paths, emits one row per violation, nothing when clean. Rows: `unmarked-mention <path>`, `blank-reason <path>`, `stale-allowlist <path>`.
  - `claude_md_block_verdict <file>` — emits `no-file`, `marker-count <begin|end> <n>`, `marker-order`, `empty-block`, `missing-literal <literal>`.

- [ ] **Step 1: Add the tables**

Insert immediately before the `# Check 7 — the self-tests` comment block (currently line 4595):

```bash
# ---------------------------------------------------------------------------------------------
# Check 12 (definitions) — the ciReport corpus freeze and the CLAUDE.md procedure block (SMA-597).
#
# WHY A FILE-SET RULE AND NOT A PATTERN. SMA-554 records the controlled comparison: this file's
# pattern-matched check was bypassed FOUR times in review, the exact-literal one next to it once.
# A literal has no tail to enumerate. The token `ciReport` appears in zero tracked files outside
# docs/ (measured), which is what makes a bare-token rule viable.
#
# THREE WAYS TO BE CLEAN, and the order matters for the error message: a `superseded` marker (a
# dated record, annotated in place by SMA-597), an `ok` marker (a deliberate reference to the
# CORRECTED procedure), or an allowlist row. The markers are what keep this table at three
# entries: without them every future document referencing the corrected procedure would need a
# row here, the gate's false-positive rate for correct mentions would be 100%, and the habit that
# trains — "red, add a row, move on" — admits the broken advice just as readily.
#
# `path|reason` strings, NOT an associative array: macOS ships bash 3.2, which has no `declare -A`,
# and this gate runs locally as often as in CI.
CIREPORT_MENTIONS_ALLOWED=(
  # The gate's own two files: run.sh must contain the token because it IS the search pattern, and
  # the README must contain it to document check 12. Consequence, recorded in the README's
  # Limitations: check 12 is structurally blind to the token in these two files. The alternatives
  # were worse — obfuscating the pattern (`ci''Report`) defeats the literal-set argument and is
  # the fragile-comment class run.sh:92-93 already warns about, and excluding ci/** creates a free
  # bypass via any ci/**/README.md.
  "ci/actionlint/run.sh|the gate's own search pattern"
  "ci/actionlint/README.md|the gate's own documentation"
  # CLAUDE.md carries the corrected procedure itself. A marker here would be the authority
  # self-certifying; assertion B (claude_md_block_verdict) is what actually guards this file.
  "CLAUDE.md|the corrected procedure itself — the authority does not self-certify"
)

# The five load-bearing elements of the CLAUDE.md block. Presence only — this cannot tell a
# correct jq from a subtly wrong one (README Limitations L2). Each earns its place: the first two
# are the finding (the real exit code is in operations[], on the task-execution entry), the next
# two are where the output actually lives, and lastRunTime is the cross-check that stops a reader
# pairing one run's command with another run's output.
DOC_DIAGNOSIS_REQUIRED_LITERALS=(
  'operations[]'
  'task-execution'
  '.moon/cache/states/'
  'stderr.log'
  'lastRunTime'
)
```

- [ ] **Step 2: Add `doc_diagnosis_verdict`**

Directly below the tables:

```bash
# Emits one row per violation, nothing when clean. Takes a FILE listing candidate paths, one per
# line, rather than running `git ls-files` itself — that split is what lets the self-test drive it
# against temp fixtures with no git repo (SMA-597).
doc_diagnosis_verdict() {
  local list="$1" f entry path reason found
  [ -f "$list" ] && [ -r "$list" ] || { echo "no-list"; return; }

  while IFS= read -r f; do
    [ -n "$f" ] || continue
    found=0
    for entry in "${CIREPORT_MENTIONS_ALLOWED[@]}"; do
      path="${entry%%|*}"
      reason="${entry#*|}"
      if [ "$path" = "$f" ]; then
        found=1
        # A blank reason is an assertion failure in its own right — the same rule T_EXEMPT and
        # ALLOW_NO_CARGO_BACKING carry in ci_targets.py. An unexplained waiver is not a waiver.
        [ -n "$reason" ] || echo "blank-reason $f"
        break
      fi
    done
    [ "$found" -eq 1 ] && continue
    if [ -r "$f" ] && grep -q 'moon-diagnosis:superseded\|moon-diagnosis:ok' "$f"; then
      continue
    fi
    echo "unmarked-mention $f"
  done < "$list"

  # Stale entries: an allowlist row naming a file that is gone, or no longer carries the token.
  # Non-fatal — the table should shrink, and reporting is what prompts that. Check 8e's
  # `stale-skip` and COE_SKIP's line+text keying are the precedents for not letting a hatch rot.
  for entry in "${CIREPORT_MENTIONS_ALLOWED[@]}"; do
    path="${entry%%|*}"
    if [ ! -r "$path" ] || ! grep -q 'ciReport' "$path" 2>/dev/null; then
      echo "stale-allowlist $path"
    fi
  done
}
```

- [ ] **Step 3: Add `claude_md_block_verdict`**

```bash
# Assertion B. Marker integrity AND required content — non-empty alone is satisfied by a single
# space or a TODO, which is exactly the likeliest real failure (someone trims CLAUDE.md and leaves
# the markers). ci-targets does not stop at markers either: parse_doc_targets enforces
# count/order/non-empty and compare_doc_targets then asserts the region mirrors T verbatim. This
# is the same shape, one notch weaker (containment, not verbatim), because the block is prose.
claude_md_block_verdict() {
  local file="$1" nb ne lb le block lit
  [ -f "$file" ] && [ -r "$file" ] || { echo "no-file"; return; }

  nb="$(grep -c 'moon-diagnosis:begin' "$file" || true)"
  ne="$(grep -c 'moon-diagnosis:end' "$file" || true)"
  [ "$nb" -eq 1 ] || echo "marker-count begin $nb"
  [ "$ne" -eq 1 ] || echo "marker-count end $ne"
  [ "$nb" -eq 1 ] && [ "$ne" -eq 1 ] || return

  lb="$(grep -n 'moon-diagnosis:begin' "$file" | cut -d: -f1)"
  le="$(grep -n 'moon-diagnosis:end' "$file" | cut -d: -f1)"
  if [ "$lb" -ge "$le" ]; then echo "marker-order"; return; fi

  block="$(sed -n "$((lb + 1)),$((le - 1))p" "$file")"
  if [ -z "$(printf '%s' "$block" | tr -d '[:space:]')" ]; then echo "empty-block"; return; fi

  for lit in "${DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}"; do
    printf '%s' "$block" | grep -qF -- "$lit" || echo "missing-literal $lit"
  done
}
```

- [ ] **Step 4: Syntax-check the file**

```bash
bash -n ci/actionlint/run.sh
```

Expected: no output, exit 0.

- [ ] **Step 5: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "ci(repo): add check 12's tables and verdict functions (SMA-597)"
```

---

## Task 4: Check 12's self-test table

**Files:**
- Modify: `ci/actionlint/run.sh` (`doc_diagnosis_self_test()`; `run_self_tests` call; `SELF_TEST_COUNT` at :48; the usage text at :66; the "All THIRTEEN" comment at :4597)

**Interfaces:**
- Consumes: `doc_diagnosis_verdict`, `claude_md_block_verdict` from Task 3.
- Produces: `doc_diagnosis_self_test` — increments `SELF_TESTS_RAN`. `SELF_TEST_COUNT` becomes 14.

- [ ] **Step 1: Write the failing self-test**

Insert directly after `release_plan_self_test`'s definition (ends near line 4593), before the check 7 comment block:

```bash
doc_diagnosis_self_test() {
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
  local rc=0 tmpd list got

  expect_doc() {
    local name="$1" expected="$2"
    if [ "$got" != "$expected" ]; then
      fail "doc-diagnosis self-test '$name': got '$got', expected '$expected'. Check 12 is not
      deciding what it is documented to decide."
      rc=1
    fi
  }

  tmpd="$(mktemp -d)"
  list="$tmpd/list"

  # --- doc_diagnosis_verdict ---------------------------------------------------------------
  # A file carrying the token and NO marker is the violation this check exists for.
  printf 'see ciReport.json for details\n' > "$tmpd/new-plan.md"
  printf '%s\n' "$tmpd/new-plan.md" > "$list"
  got="$(doc_diagnosis_verdict "$list" | grep -v '^stale-allowlist ')"
  expect_doc 'an unmarked mention fires' "unmarked-mention $tmpd/new-plan.md"

  # A superseded marker clears it — the 67 historical documents.
  printf 'see ciReport.json\n<!-- moon-diagnosis:superseded -->\n' > "$tmpd/old-plan.md"
  printf '%s\n' "$tmpd/old-plan.md" > "$list"
  got="$(doc_diagnosis_verdict "$list" | grep -v '^stale-allowlist ')"
  expect_doc 'a superseded marker clears it' ''

  # An ok marker clears it — a deliberate reference to the corrected procedure.
  printf 'see ciReport.json\n<!-- moon-diagnosis:ok -->\n' > "$tmpd/correct.md"
  printf '%s\n' "$tmpd/correct.md" > "$list"
  got="$(doc_diagnosis_verdict "$list" | grep -v '^stale-allowlist ')"
  expect_doc 'an ok marker clears it' ''

  # A removed offender is NOT a failure — subset, not equality. The set should only ever shrink,
  # and making a cleanup red the gate that authorised it pushes people toward loosening the gate.
  : > "$list"
  got="$(doc_diagnosis_verdict "$list" | grep -v '^stale-allowlist ')"
  expect_doc 'an empty corpus emits no violation rows' ''

  # A missing list is infrastructure, not a clean repo.
  got="$(doc_diagnosis_verdict "$tmpd/nope" | grep -v '^stale-allowlist ')"
  expect_doc 'a missing list reports no-list' 'no-list'

  # --- claude_md_block_verdict -------------------------------------------------------------
  local good="prose
<!-- moon-diagnosis:begin -->
read .operations[] and pick task-execution
output is in .moon/cache/states/<p>/<t>/stderr.log
cross-check lastRunTime
<!-- moon-diagnosis:end -->
more prose"

  printf '%s\n' "$good" > "$tmpd/claude.md"
  got="$(claude_md_block_verdict "$tmpd/claude.md")"
  expect_doc 'a complete block is clean' ''

  got="$(claude_md_block_verdict "$tmpd/absent.md")"
  expect_doc 'a missing file reports no-file' 'no-file'

  # A duplicated marker: CLAUDE.md's own ci-targets entry warns that a second copy anywhere in the
  # file — even inside backticks in prose — breaks the count. Same hazard here.
  printf '%s\n<!-- moon-diagnosis:begin -->\n' "$good" > "$tmpd/dup.md"
  got="$(claude_md_block_verdict "$tmpd/dup.md")"
  expect_doc 'a duplicated begin marker fires' 'marker-count begin 2'

  printf '<!-- moon-diagnosis:end -->\nx\n<!-- moon-diagnosis:begin -->\n' > "$tmpd/order.md"
  got="$(claude_md_block_verdict "$tmpd/order.md")"
  expect_doc 'markers out of order fire' 'marker-order'

  printf '<!-- moon-diagnosis:begin -->\n   \n<!-- moon-diagnosis:end -->\n' > "$tmpd/empty.md"
  got="$(claude_md_block_verdict "$tmpd/empty.md")"
  expect_doc 'a whitespace-only block fires' 'empty-block'

  # Each required literal deleted in turn — driven from the array, so a sixth entry is covered
  # automatically rather than needing a new fixture.
  local lit stripped
  for lit in "${DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}"; do
    stripped="$(printf '%s\n' "$good" | grep -vF -- "$lit")"
    printf '%s\n' "$stripped" > "$tmpd/miss.md"
    got="$(claude_md_block_verdict "$tmpd/miss.md")"
    expect_doc "required literal '$lit' deleted fires" "missing-literal $lit"
  done

  rm -rf "$tmpd"
  return "$rc"
}
```

- [ ] **Step 2: Run it and verify it fails on the count**

```bash
ci/actionlint/run.sh --self-test 2>&1 | tail -5
```

Expected: FAIL — `self-test definitions: 14 '*_self_test' functions are defined but SELF_TEST_COUNT is 13`. This proves check 7's definition-count assertion is live, which is the thing that would otherwise let a table be added and never called.

- [ ] **Step 3: Wire the call and bump the count**

Add to `run_self_tests`, after `release_plan_self_test`:

```bash
  doc_diagnosis_self_test
```

Change line 48 from `SELF_TEST_COUNT=13` to:

```bash
SELF_TEST_COUNT=14  # extractor, path-filter, branch-filter, config, ci-target-floor,
                    # invocation-allowlist, affected-graph-wiring, block-execution,
                    # kill-predicate, affected-smoke-block, release-guard, cargo-lock-step,
                    # release-plan, doc-diagnosis
```

Update the usage text at :66 — `the thirteen fixture tables` → `the fourteen fixture tables`, and add `doc-diagnosis` to the enumeration ending `..., cargo-lock step, release-plan.`

Update the comment at :4597 — `All THIRTEEN are defined above` → `All FOURTEEN are defined above`.

- [ ] **Step 4: Run the self-tests and verify they pass**

```bash
ci/actionlint/run.sh --self-test 2>&1 | tail -5; echo "rc=$?"
```

Expected: no failures, `rc=0`.

- [ ] **Step 5: Prove the new table is not decorative**

Delete the `doc_diagnosis_self_test` call from `run_self_tests`, re-run, restore.

```bash
ci/actionlint/run.sh --self-test 2>&1 | grep 'self-test counter'
```

Expected: `self-test counter: 13 of 14 self-tests ran.` Restore the line afterwards and re-run to confirm clean.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "ci(repo): add check 12's self-test table and bump the count to 14 (SMA-597)"
```

---

## Task 5: Check 12's production block and arity floors

**Files:**
- Modify: `ci/actionlint/run.sh` (production block, inserted after check 11's routing and before `selftest_mutation_battery` near line 5558)

**Interfaces:**
- Consumes: `doc_diagnosis_verdict`, `claude_md_block_verdict`, both tables.
- Produces: three column-0 lines Task 6 pins — the corpus floor, the literals floor, and `done < <(doc_diagnosis_verdict "$DD_LIST")`.

- [ ] **Step 1: Add the production block**

Insert immediately before `selftest_mutation_battery`:

```bash
# ---------------------------------------------------------------------------------------------
# Check 12 — the ciReport corpus freeze and the CLAUDE.md procedure block (SMA-597). Runs here,
# not in --self-test, because it reads the real tracked tree, like checks 5/6/10/11.
#
# THE ARITY FLOORS ARE PART OF THE CHECK. doc_diagnosis_verdict iterates a corpus, so an EMPTY
# corpus emits zero rows and the gate passes having asserted nothing — `∅ ⊆ allowlist`. That is
# the same class ci_targets.py records as MEASURED for check 8e's emptied table, and the subset
# rule (deliberate: the corpus should only ever shrink) removes the control that would otherwise
# catch it. `infra`, not `fail`: a corpus that vanished is a broken gate, not a clean repo.
#
# `-ge 60` rather than `-eq 67` so genuine cleanup of a handful of documents needs no re-baseline.
#
# CORPUS COMMAND: unfiltered `git ls-files`, no pathspec. `git ls-files -- 'docs/**/*.md'` matches
# ZERO top-level docs/*.md files — git matches `**` without FNM_PATHNAME, so the literal `/` is
# still required (MEASURED: docs/dev-setup.md is tracked and unmatched). CLAUDE.md records the
# same trap for repo:ruff-ci's corpus. Scanning the whole tracked tree has no pathspec to get
# wrong and closes the docs/anything.md bypass at the same time. It reads the INDEX, so a file
# written but not yet `git add`ed is invisible — a local run can be green where CI is red.
#
# COLUMN 0 for the two floors and the read loop: ACTIONLINT_SH_CALL_SITES matches with no leading
# whitespace, so indenting any of them reds that pin rather than silently satisfying it.
# ---------------------------------------------------------------------------------------------
DD_LIST="$(mktemp)"
git ls-files -z | xargs -0 grep -l 'ciReport' 2>/dev/null | sort > "$DD_LIST" || true
DD_N="$(wc -l < "$DD_LIST" | tr -d '[:space:]')"

[ "$DD_N" -ge 60 ] || infra "check 12: the corpus command found $DD_N files carrying the token, expected at least 60 — it has probably stopped matching, and an empty corpus would pass this check having asserted nothing"
[ "${#DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}" -ge 5 ] || infra "check 12: DOC_DIAGNOSIS_REQUIRED_LITERALS has ${#DOC_DIAGNOSIS_REQUIRED_LITERALS[@]} entries, expected at least 5"

while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-list)
      infra "check 12: could not build the corpus list — git ls-files failed." ;;
    unmarked-mention\ *)
      fail "check 12: ${verdict#unmarked-mention } mentions ciReport.json but carries neither
      <!-- moon-diagnosis:superseded --> nor <!-- moon-diagnosis:ok -->, and is not in
      CIREPORT_MENTIONS_ALLOWED. If it reproduces the broken advice (no action-level exitCode; the
      file has no stdout/stderr), fix it against CLAUDE.md's moon-diagnosis block. If it is a
      correct reference, add the :ok marker. If it is a historical record, add :superseded." ;;
    blank-reason\ *)
      fail "check 12: the CIREPORT_MENTIONS_ALLOWED entry for ${verdict#blank-reason } has an
      empty reason. An unexplained waiver is not a waiver." ;;
    stale-allowlist\ *)
      echo "actionlint gate: check 12 NOTE: CIREPORT_MENTIONS_ALLOWED names ${verdict#stale-allowlist }, which is gone or no longer carries the token — drop the row." >&2 ;;
    *)
      infra "check 12: unrecognised verdict '$verdict'" ;;
  esac
done < <(doc_diagnosis_verdict "$DD_LIST")

rm -f "$DD_LIST"

while IFS= read -r verdict; do
  case "$verdict" in
    '') ;;
    no-file)
      fail "check 12: CLAUDE.md does not exist or is unreadable, so the corrected diagnosis
      procedure cannot be confirmed present." ;;
    marker-count\ *)
      fail "check 12: CLAUDE.md has the wrong number of moon-diagnosis markers ($verdict).
      Exactly one begin and one end are required — a second copy anywhere in the file, even
      inside backticks in prose, breaks this the same way it breaks ci-targets." ;;
    marker-order)
      fail "check 12: CLAUDE.md's moon-diagnosis:end precedes its begin." ;;
    empty-block)
      fail "check 12: CLAUDE.md's moon-diagnosis block is empty. Deleting the procedure would
      switch off the correction this gate exists to protect." ;;
    missing-literal\ *)
      fail "check 12: CLAUDE.md's moon-diagnosis block no longer contains
      '${verdict#missing-literal }'. Every entry in DOC_DIAGNOSIS_REQUIRED_LITERALS is a
      load-bearing element of the measured procedure." ;;
    *)
      infra "check 12: unrecognised block verdict '$verdict'" ;;
  esac
done < <(claude_md_block_verdict CLAUDE.md)
```

- [ ] **Step 2: Run the full gate**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh; echo "rc=$?"
```

Expected: `rc=0`. A `check 12` failure here means Task 2's annotation is incomplete — fix that, not this.

- [ ] **Step 3: Prove check 12 can red (new offender)**

```bash
printf 'diagnose via ciReport.json\n' > docs/superpowers/plans/zz-probe.md
git add docs/superpowers/plans/zz-probe.md
ci/actionlint/run.sh 2>&1 | grep 'check 12'; echo "rc=${PIPESTATUS[0]}"
git rm -q -f docs/superpowers/plans/zz-probe.md
```

Expected: an `unmarked-mention docs/superpowers/plans/zz-probe.md` failure. This is the acceptance evidence for AC3.

- [ ] **Step 4: Prove check 12 can red (deleted procedure)**

```bash
cp CLAUDE.md /tmp/claude.bak
python3 - <<'PY'
import re, pathlib
p = pathlib.Path("CLAUDE.md")
t = p.read_text()
t = re.sub(r'(moon-diagnosis:begin -->).*?(<!-- moon-diagnosis:end)', r'\1\n\2', t, flags=re.S)
p.write_text(t)
PY
ci/actionlint/run.sh 2>&1 | grep 'check 12'
cp /tmp/claude.bak CLAUDE.md
```

Expected: `empty-block` failure. Restore and re-run to confirm clean.

- [ ] **Step 5: Prove the corpus floor can red**

Temporarily change `-ge 60` to `-ge 600`, run, restore.

Expected: `INFRASTRUCTURE ERROR: check 12: the corpus command found 69 files ... expected at least 600`, exit 2 — **not** exit 1. The distinction is the point.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "ci(repo): run check 12 against the real tree, with arity floors (SMA-597)"
```

---

## Task 6: Pin check 12's production lines

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`ACTIONLINT_SH_CALL_SITES`, the tuple ending near line 790)

**Interfaces:**
- Consumes: the three column-0 lines from Task 5.
- Produces: nothing downstream.

- [ ] **Step 1: Add the three entries**

Append to `ACTIONLINT_SH_CALL_SITES`, before its closing paren:

```python
    # SMA-597 — check 12's production call site, at run.sh's top level (verified: not nested in a
    # function, if, or loop) — column 0 like every other entry above. Same shape as checks 8b-8e:
    # `doc_diagnosis_verdict` is also called from inside its own self-test fixtures, so a
    # substring test would be satisfied by those and survive deleting this exact production line,
    # leaving check 7's counter, check 9's battery and SELF_TEST_COUNT all green while the corpus
    # stops being guarded.
    'done < <(doc_diagnosis_verdict "$DD_LIST")',
    # ...and the CORPUS arity floor, the vacuous-pass guard. doc_diagnosis_verdict iterates a
    # corpus, so an empty one emits zero rows and check 12 passes having asserted nothing —
    # `∅ ⊆ allowlist`. Check 12 uses a SUBSET rule deliberately (the corpus should only ever
    # shrink), which removes the strict-equality control that would otherwise catch this, so the
    # floor is the only thing standing in its place. Same reasoning as check 8e's two floors above.
    '[ "$DD_N" -ge 60 ] || infra "check 12: the corpus command found $DD_N files carrying the token, expected at least 60 — it has probably stopped matching, and an empty corpus would pass this check having asserted nothing"',
    # ...and the REQUIRED-LITERALS floor. Assertion B iterates its table too: empty it and the
    # CLAUDE.md block passes on "markers present and non-empty" alone, which a single space
    # satisfies — the likeliest real failure being an unrelated CLAUDE.md trim that leaves the
    # markers behind.
    '[ "${#DOC_DIAGNOSIS_REQUIRED_LITERALS[@]}" -ge 5 ] || infra "check 12: DOC_DIAGNOSIS_REQUIRED_LITERALS has ${#DOC_DIAGNOSIS_REQUIRED_LITERALS[@]} entries, expected at least 5"',
```

- [ ] **Step 2: Verify the pins match the real file**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python ci/affected-graph/ci_targets.py --self-test 2>&1 | tail -5
```

Expected: pass. A failure naming one of the three lines means a byte differs — copy it verbatim from `run.sh` rather than retyping.

- [ ] **Step 3: Prove the pin can red**

Indent the `done < <(doc_diagnosis_verdict "$DD_LIST")` line by two spaces, re-run step 2, then restore.

Expected: failure naming that line. This proves the column-0 rule is live.

- [ ] **Step 4: Lint the Python**

```bash
moon run repo:ruff-ci
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "ci(repo): pin check 12's production call site and both floors (SMA-597)"
```

---

## Task 7: The CI diagnostics artifact

**Files:**
- Modify: `.github/workflows/ci.yml` (new step after the `moon ci` step)

**Interfaces:**
- Consumes: nothing.
- Produces: the `moon-diagnostics` artifact the CLAUDE.md block's CI note points at.

- [ ] **Step 1: Add the upload step**

Immediately after the `moon ci (affected graph)` step, mirroring the existing `nextest-junit` upload:

```yaml
      # SMA-597: ciReport.json carries the failing task's command and real exit code, and the
      # per-task logs under states/ carry the only copy of its output. Neither survives the
      # runner. Without this, the documented diagnosis procedure is unusable for a red CI check —
      # the case it most needs to serve. `if: failure()` so green runs upload nothing.
      - name: Upload moon diagnostics
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: moon-diagnostics
          path: |
            .moon/cache/ciReport.json
            .moon/cache/states/**/stdout.log
            .moon/cache/states/**/stderr.log
            .moon/cache/states/**/lastRun.json
          if-no-files-found: ignore
          retention-days: 7
```

- [ ] **Step 2: Verify the workflow still lints**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh; echo "rc=$?"
```

Expected: `rc=0`. Checks 8/8b/8d read `ci.yml`'s step structure, so a malformed step reds here.

- [ ] **Step 3: Confirm the pinned `actions/upload-artifact` version matches the repo's existing use**

```bash
grep -n 'upload-artifact' .github/workflows/*.yml
```

Expected: the new step uses the same major version as the existing `nextest-junit` upload. If they differ, match the existing one.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(repo): upload moon diagnostics on a failed CI run (SMA-597)"
```

---

## Task 8: Documentation and the stale counts

**Files:**
- Modify: `ci/actionlint/README.md`
- Modify: `moon.yml` (comment block only)
- Modify: `CLAUDE.md` (the SELF_TEST_COUNT prose)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: Add check 12's README row**

In the check table, after row 11:

```markdown
| 12 | Every tracked file carrying the token `ciReport` must carry `<!-- moon-diagnosis:superseded -->` (a dated record), `<!-- moon-diagnosis:ok -->` (a deliberate reference to the corrected procedure), or a `CIREPORT_MENTIONS_ALLOWED` row with a non-empty reason — plus CLAUDE.md's `moon-diagnosis` block must exist, have exactly one ordered marker pair, be non-empty, and contain all five entries of `DOC_DIAGNOSIS_REQUIRED_LITERALS` (SMA-597) |
```

- [ ] **Step 2: Re-count the thirteens**

Update `:33` (row 7 — add `doc-diagnosis` to the enumeration, "all thirteen" → "all fourteen", "a fourteenth table" → "a fifteenth table"), `:40` (row 9, "thirteen self-test invocations" → "fourteen"), `:669`, `:677-678` ("thirteen fixture tables, thirteen mutants" → fourteen/fourteen, and "fourteen concurrent subprocesses" → fifteen), `:701`, `:704`.

Then `moon.yml` at `:667`, `:671`, `:686` and `CLAUDE.md` at `:308-311` (add "and SMA-597 the fourteenth, `doc_diagnosis_self_test` at check 12").

Verify none survive:

```bash
grep -rn 'thirteen\|THIRTEEN' ci/actionlint/ moon.yml | grep -iv 'sma-603\|release_plan'
```

Expected: no rows describing the self-test count. (Historical references to SMA-603 adding *the* thirteenth are correct and stay.)

- [ ] **Step 3: Amend the README's "## Why"**

Its premise is entirely about workflow `paths:` filters going dead, which does not explain a docs-corpus check. Append:

```markdown
Checks 8b–8f, 10–11 and 12 are not about workflow filters. They live here because this gate
declares `inputs: ['**/*']` and therefore runs on every PR, which is the reachability a
cross-cutting pin needs — a narrower `inputs` list would be the SMA-553 failure class. Check 12
(SMA-597) is the clearest case: a docs-corpus freeze has to see the PR that adds a new document.
```

- [ ] **Step 4: Add the Limitations entries**

```markdown
- **L13 (SMA-597).** Check 12 gates the PRESENCE of the procedure's five load-bearing literals,
  not its correctness. Editing the `jq` inside CLAUDE.md's block into something subtly wrong stays
  green. Closing this needs a gate that EXECUTES the procedure against a deliberately failed task;
  that is a follow-up issue, not scope here.
- **L14 (SMA-597).** Check 12 is structurally blind to the token in its own two files
  (`ci/actionlint/run.sh`, `ci/actionlint/README.md`), both allowlisted because run.sh must contain
  the search pattern and the README must document it. Broken advice written into the gate's own
  source is invisible to it.
- **L15 (SMA-597).** The corpus reads the git INDEX (`git ls-files`), so a file written but not yet
  `git add`ed is invisible. A local run can be green where CI is red.
```

- [ ] **Step 5: Re-measure check 9's timing table**

The battery goes from 14 to 15 concurrent `--self-test` subprocesses, invalidating the table at `README.md:661-667`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
time ci/actionlint/run.sh > /dev/null 2>&1
```

Record the new wall-clock figure in that table and note the row count is now fifteen.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/README.md moon.yml CLAUDE.md
git commit -m "docs(ci): document check 12 and re-count the self-test tables (SMA-597)"
```

---

## Task 9: Full-graph acceptance

**Files:** none modified.

- [ ] **Step 1: Run the gate graph exactly as CI does**

Run the command between CLAUDE.md's `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` markers verbatim, with the proto shims on PATH.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  --base origin/main --include-relations
```

Expected: all PASS. Pay attention to `repo:actionlint` (checks 7, 9 and 12) and `repo:affected-smoke` (the three new pins).

If a task fails without attribution, **use the procedure this issue just wrote** — that is itself acceptance evidence for AC1. Note the three `release-parity*` gates abort inconclusive at rc 2 inside an agent session unless `AI_AGENT`, `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` are unset.

- [ ] **Step 2: Confirm the working tree is clean**

```bash
git status --short
```

Expected: no output. Any probe file from Task 5 must already be gone.

- [ ] **Step 3: Confirm moon.yml has no leftover probe tasks**

```bash
git diff origin/main -- moon.yml
```

Expected: comment-block changes only (the count re-wording). No `diagnose-probe`, `ow-probe`, `m-fail`, `m-nocmd` or `m-cached` task.

---

## Self-Review

**Spec coverage.** §2 → Task 1. §7's supersession decision → Task 2. §3.2 (corpus, markers, allowlist, floor) → Tasks 3 and 5. §3.3 (Assertion B, literals) → Tasks 3 and 5. §3.4 (self-test) → Task 4. §3.5 (the three pins) → Task 6. §3.6 (stale rows) → Task 3 step 2. §1.10 (CI artifact) → Task 7. §4's file table → Tasks 1–8, all seven files covered. §5's L2/L4 and the index caveat → Task 8 step 4. AC1's "demonstrated" → Task 9 step 1; AC2 → recorded in Task 1's block; AC3 → Task 5 step 3; AC4 → Task 1 steps 1–2.

**Placeholder scan.** No TBDs. Every code step carries the literal text to insert; every verification step names the command and its expected output.

**Type consistency.** `doc_diagnosis_verdict` takes a list-file in Task 3, Task 4's fixtures, and Task 5's production call. `claude_md_block_verdict` takes a file path in all three. Row vocabulary (`unmarked-mention`, `blank-reason`, `stale-allowlist`, `no-list`, `no-file`, `marker-count`, `marker-order`, `empty-block`, `missing-literal`) is identical across Tasks 3, 4 and 5. `DD_LIST`/`DD_N` are introduced in Task 5 and pinned verbatim in Task 6. `SELF_TEST_COUNT` reaches 14 in Task 4 and is re-described in Task 8.

**One ordering constraint worth stating.** Task 2 must precede Task 5: check 12's production run reads the real tree, and until the 67 documents carry their marker it reds with 67 rows. Tasks 3 and 4 are pure and can be done in any order relative to Task 2.
