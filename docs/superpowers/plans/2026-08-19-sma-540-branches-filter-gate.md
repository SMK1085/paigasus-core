# SMA-540 — `branches:` filter gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the SMA-525 `repo:actionlint` gate so a typo'd `branches:` filter reds CI instead of silently and permanently disabling a workflow.

**Architecture:** Reuse the five layers `ci/actionlint/run.sh` already has — extractor → pure verdict function → record scanner → message call site → self-test tables — adding a `branches`/`branches-ignore` key family rather than a parallel stack. The five existing `branches: [main]` filters move to block style so the extractor can read them without growing any YAML-parsing surface. A wildcard-free entry must resolve as `refs/remotes/origin/<name>` or appear in a documented `BRANCH_SKIP` array.

**Tech Stack:** bash 3.2 (macOS) / bash 5 (CI), POSIX awk, git plumbing (`for-each-ref`, `check-ref-format`), actionlint 1.7.12 via proto, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-19-sma-540-branches-filter-gate-design.md` — read it before starting. Decisions are cited below as D1–D10.

## Global Constraints

- **`repo:actionlint` runs inside `CI / moon ci`, this repo's ONLY required status check.** A false red wedges every merge, including the PR that would fix it. When in doubt, prefer under-covering to a check that can red on a clean tree.
- **PATH:** every shell command needs `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims FIRST) or `moon`/`actionlint` resolve to the wrong version or not at all.
- **Work in the worktree** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-540` on branch `feature/sma-540-branches-filter-gate`. Never `cd` to the main checkout — a peer session is live there on a different branch.
- **Exit codes:** `1` = assertion failure (`fail`), `2` = infrastructure error (`infra`). Never introduce a third.
- **`run.sh` is not `set -e`** — several checks deliberately inspect non-zero exits. Capture status explicitly; set `FAILED` via `fail`.
- **`infra` inside `$( )` exits only the subshell.** `pattern_verdict`/`branch_verdict` are always called in command substitution, so they must *return a token* and let the main-shell call site do the `infra`. This is why D7's canary is a token, not a direct `infra`.
- **Commit messages:** conventional commits, scope `repo`. Subject lowercase after the scope, ≤100 chars. **No body line may begin with `word:`** — commitlint parses it as a trailer and fails `footer-leading-blank`. Write "SMA-540" not "#540".
- **CLAUDE.md edits must be prose only** — no `ci-targets` marker strings, no pasted `moon ci …` command. SMA-541 is in flight and adds a gate that counts those markers; a second copy reds it (spec §6).
- **Do not touch** `pattern_verdict`, the `PATTERN` record shape, `path_filter_self_test`, or `ci.yml`'s `T=(…)` array. AC-3.
- Every new file opens with `# SPDX-License-Identifier: Apache-2.0`. (No new files are expected in this plan.)

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `.github/workflows/ci.yml` | required-check trigger | 2 filters → block style |
| `.github/workflows/prebuild.yml` | 7-platform napi verification trigger | 2 filters → block style |
| `.github/workflows/security-scan.yml` | scheduled advisory scan trigger | 1 filter → block style |
| `ci/actionlint/run.sh` | the whole gate | extractor vocabulary, `branch_verdict`, scanner dispatch, messages, self-test table |
| `ci/actionlint/README.md` | operator-facing docs | checks table rows 5–7 reworded, branch vocabulary + escape hatch sections |
| `moon.yml` | `repo:actionlint` task | `description:` no longer says "paths:" only |
| `CLAUDE.md` | repo gotchas | one prose gotcha |

---

### Task 1: Capture the AC-3 baseline, then rewrite the five filters to block style

**Files:**
- Modify: `.github/workflows/ci.yml:4-7`
- Modify: `.github/workflows/prebuild.yml:11`, `.github/workflows/prebuild.yml:26`
- Modify: `.github/workflows/security-scan.yml:24`

**Interfaces:**
- Consumes: nothing.
- Produces: `$SCRATCH/baseline.txt` (old extractor's records over the pre-rewrite files) and `$SCRATCH/pre/*.yml` (copies of the pre-rewrite workflows), both consumed by Task 2's AC-3 proof.

**Why the baseline is captured first:** the naive AC-3 proof (records before vs after) is *infeasible* — this task adds a line per filter, so every downstream `KEY` line number shifts. The sound proof is the **new** extractor against the **pre-rewrite** files, so those files must be preserved now (spec §5).

- [ ] **Step 1: Capture the pre-rewrite files and the old extractor's records**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/sma540
mkdir -p "$SCRATCH/pre"
cp .github/workflows/ci.yml .github/workflows/prebuild.yml \
   .github/workflows/security-scan.yml "$SCRATCH/pre/"

# The extractor is a self-contained awk function; lift it out and drive it over the files.
{
  sed -n '/^extract_paths_keys() {/,/^}$/p' ci/actionlint/run.sh
  echo 'for f in "$@"; do echo "== $(basename "$f")"; extract_paths_keys "$f"; done'
} > "$SCRATCH/extract_old.sh"

bash "$SCRATCH/extract_old.sh" "$SCRATCH"/pre/*.yml > "$SCRATCH/baseline.txt"
cat "$SCRATCH/baseline.txt"
```

Expected: non-empty output containing `KEY	paths` lines for `prebuild.yml` and `security-scan.yml` (`ci.yml` has no `paths:` filter). If `baseline.txt` is empty, STOP — the `sed` range failed and the proof would be vacuous.

- [ ] **Step 2: Rewrite `ci.yml`**

Replace lines 4–7:

```yaml
on:
  pull_request:
    branches: [main]
  push:
    branches: [main]
```

with:

```yaml
on:
  pull_request:
    branches:
      - main
  push:
    branches:
      - main
```

- [ ] **Step 3: Rewrite `prebuild.yml`**

Both occurrences. Line 11 (`push:`) becomes:

```yaml
  push:
    branches:
      - main
    paths:
      - 'rs/**'                              # includes rs/Cargo.lock + rs/rust-toolchain.toml
```

and line 26 (`pull_request:`) becomes:

```yaml
  pull_request:
    branches:
      - main
    paths:
      - '.github/workflows/prebuild.yml'
```

Keep every existing `paths:` entry and its trailing comment exactly as-is — only the `branches:` line changes shape.

- [ ] **Step 4: Rewrite `security-scan.yml`**

Line 24 becomes:

```yaml
  pull_request:
    branches:
      - main
    paths:
      - '.github/workflows/security-scan.yml'
```

- [ ] **Step 5: Verify the gate is unaffected**

Run: `ci/actionlint/run.sh && echo GATE-GREEN`
Expected: `GATE-GREEN`. The extractor does not yet recognise `branches`, so this is a pure no-op for the gate — a failure here means a YAML typo was introduced.

- [ ] **Step 6: Verify the YAML still parses as the same triggers**

Run: `actionlint -shellcheck= -pyflakes= && echo LINT-CLEAN`
Expected: `LINT-CLEAN`.

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/ci.yml .github/workflows/prebuild.yml .github/workflows/security-scan.yml
git commit -m "refactor(repo): write branch filters as block sequences (SMA-540)" -m "The actionlint gate reads block sequences and deliberately does not parse
the inline flow form, emitting a loud no-items failure instead. Moving the
five filters is what lets the next commit read them without growing any
YAML-parsing surface in the only required check."
```

---

### Task 2: Teach the extractor the branch key family

**Files:**
- Modify: `ci/actionlint/run.sh` — `extract_paths_keys` (rename + vocabulary), `flow_keys`, `extractor_self_test`, and the three call sites of the old name

**Interfaces:**
- Consumes: `$SCRATCH/baseline.txt`, `$SCRATCH/pre/*.yml` from Task 1.
- Produces: `extract_filter_keys <file>` — same record format, four kinds:
  `KEY\t<paths|paths-ignore|branches|branches-ignore>\t<lineno>` and `ITEM\t<kind>\t<value>`.

- [ ] **Step 1: Write the failing fixtures**

In `extractor_self_test`, add these five fixtures immediately after the `'simple block'` fixture:

```bash
  check_fixture 'a branches block is extracted' \
"$(printf 'KEY\tbranches\t4\nITEM\tbranches\tmain')" \
'name: t
on:
  push:
    branches:
      - main
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches block followed by a sibling paths block keeps both' \
"$(printf 'KEY\tbranches\t4\nITEM\tbranches\tmain\nKEY\tpaths\t6\nITEM\tpaths\trs/**')" \
'name: t
on:
  push:
    branches:
      - main
    paths:
      - "rs/**"
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches flow sequence emits KEY with no ITEMs' \
"$(printf 'KEY\tbranches\t4')" \
'name: t
on:
  push:
    branches: [main]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a branches-ignore block is extracted' \
"$(printf 'KEY\tbranches-ignore\t4\nITEM\tbranches-ignore\tdev')" \
'name: t
on:
  push:
    branches-ignore:
      - dev
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'

  check_fixture 'a workflow_dispatch input named branches is not a filter' \
"" \
'name: t
on:
  workflow_dispatch:
    inputs:
      branches:
        description: which branches
        required: false
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'
```

- [ ] **Step 2: Run the self-test to verify the new fixtures fail**

Run: `ci/actionlint/run.sh --self-test`
Expected: FAIL, exit 1, with `extractor self-test 'a branches block is extracted' mismatch` and an empty `--- actual ---`. The `branches-ignore` and `workflow_dispatch` fixtures pass already (both expect nothing yet / nothing ever).

- [ ] **Step 3: Rename the function and its three call sites**

```bash
sed -i '' 's/extract_paths_keys/extract_filter_keys/g' ci/actionlint/run.sh
grep -c 'extract_filter_keys' ci/actionlint/run.sh
```

Expected count: `4` — the definition (`run.sh:69`) and three call sites: `extractor_self_test` (`:275`), `path_filter_self_test`'s quoted-key fixture (`:1131`), and the production loop (`:1393`). If the count is not 4, inspect before continuing.

Then update the function's header comment, which still says "Extract paths:/paths-ignore: keys":

```bash
# Extract the four filter keys — paths:, paths-ignore:, branches:, branches-ignore: — and their
# sequence entries from one workflow file. Output records, TAB-separated, in file order:
#   KEY\t<kind>\t<lineno>
#   ITEM\t<kind>\t<pattern-or-branch>
# See the contract in docs/superpowers/plans/2026-08-16-sma-525-actionlint-gate.md (Task 4) and
# ci/actionlint/README.md. Every clause below has a fixture in extractor_self_test.
```

- [ ] **Step 4: Extend the block-form key vocabulary**

Replace this block (the `if (depth != 2) next` branch's key match):

```awk
      if (stripped ~ /^["\047]?paths["\047]?[ \t]*:/)        { kind = "paths" }
      else if (stripped ~ /^["\047]?paths-ignore["\047]?[ \t]*:/) { kind = "paths-ignore" }
      else next
```

with:

```awk
      # Four filter keys, matched by one pattern and then read back out of the line, rather than
      # four near-identical regexes. A quoted key ("paths":, 'branches-ignore':) is valid YAML
      # actionlint accepts; the bare-only regex silently dropped it — no KEY record, so the checks
      # skipped the filter with no message (SMA-525 round-2 review finding A). Whitespace before
      # the colon (`paths :`) gets the same tolerance as `on :` above, for the same reason.
      if (stripped !~ /^["\047]?(paths|branches)(-ignore)?["\047]?[ \t]*:/) next
      kind = stripped
      sub(/["\047]?[ \t]*:.*$/, "", kind)   # drop the colon, any value, any trailing comment
      sub(/^["\047]/, "", kind)             # drop a leading quote
```

Then replace the block-open strip a few lines below:

```awk
      sub(/^["\047]?paths(-ignore)?["\047]?[ \t]*:/, "", rest)
```

with:

```awk
      sub(/^["\047]?(paths|branches)(-ignore)?["\047]?[ \t]*:/, "", rest)
```

- [ ] **Step 5: Extend `flow_keys` to the same vocabulary**

Replace the two `match(rest, ...)` blocks inside `flow_keys`:

```awk
          if (match(rest, /^["\047]?paths-ignore["\047]?[ \t]*:/)) {
            if (depth == target) print "KEY\tpaths-ignore\t" lineno
            i += RLENGTH - 1   # the for loop own i++ then makes the net advance RLENGTH
            continue
          }
          if (match(rest, /^["\047]?paths["\047]?[ \t]*:/)) {
            if (depth == target) print "KEY\tpaths\t" lineno
            i += RLENGTH - 1
            continue
          }
```

with a single block covering all four kinds:

```awk
          if (match(rest, /^["\047]?(paths|branches)(-ignore)?["\047]?[ \t]*:/)) {
            fkey = substr(rest, 1, RLENGTH)
            sub(/["\047]?[ \t]*:$/, "", fkey)
            sub(/^["\047]/, "", fkey)
            if (depth == target) print "KEY\t" fkey "\t" lineno
            i += RLENGTH - 1   # the for loop own i++ then makes the net advance RLENGTH
            continue
          }
```

Add `fkey` to `flow_keys`'s local-variable list so it does not leak into the global awk namespace — the signature becomes:

```awk
    function flow_keys(v, lineno, target,    depth, i, n, c, instr, qc, prevc, rest, fkey) {
```

- [ ] **Step 6: Update the six existing fixtures whose expectations change**

Each now sees a level-2 `branches:` the extractor previously ignored. These are correct-by-design, not regressions:

| Fixture | Old expected | New expected |
|---|---|---|
| `dedent closes the block` | `KEY paths 4` + `ITEM paths a/**` | append `KEY	branches	6` |
| `a paths: line inside a run block is ignored` | `""` | `KEY	branches	4` |
| `a flow-mapping event with paths emits KEY with no ITEMs` | `KEY paths 3` | `KEY	branches	3` **then** `KEY	paths	3` |
| `a flow-mapping event with paths-ignore emits KEY with no ITEMs` | `KEY paths-ignore 3` | `KEY	branches	3` **then** `KEY	paths-ignore	3` |
| `a flow-mapping event without a path filter is not a KEY` | `""` | `KEY	branches	3` |
| `a flow mapping on on: itself emits KEY with no ITEMs` | `KEY paths 2` | `KEY	branches	2` **then** `KEY	paths	2` |

The branches KEY comes **first** in the flow-mapping cases because `flow_keys` scans the line left to right and `branches` is written first in each fixture.

Rename the fifth fixture — its name becomes false:

```bash
  check_fixture 'a flow-mapping event with only a branch filter still emits a KEY' \
"$(printf 'KEY\tbranches\t3')" \
'name: t
on:
  push: { branches: [main] }   # no paths filter here
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
'
```

- [ ] **Step 7: Run the self-test to verify it passes**

Run: `ci/actionlint/run.sh --self-test && echo SELFTEST-GREEN`
Expected: `SELFTEST-GREEN`.

- [ ] **Step 8: Run the full gate**

Run: `ci/actionlint/run.sh && echo GATE-GREEN`
Expected: `GATE-GREEN`. Branch records now flow into `scan_workflow_records`, but nothing consumes them yet: `key_items` becomes 1 so `no-items` cannot fire, and `all-negated` is still guarded on `paths`. A red here means the vocabulary change caught something unintended — investigate, do not proceed.

- [ ] **Step 9: Prove AC-3 — paths records over the pre-rewrite files are unchanged**

```bash
SCRATCH=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/sma540
{
  sed -n '/^extract_filter_keys() {/,/^}$/p' ci/actionlint/run.sh
  echo 'for f in "$@"; do echo "== $(basename "$f")"; extract_filter_keys "$f"; done'
} > "$SCRATCH/extract_new.sh"

bash "$SCRATCH/extract_new.sh" "$SCRATCH"/pre/*.yml \
  | awk -F'\t' '$1 ~ /^==/ || $2 == "paths" || $2 == "paths-ignore"' \
  > "$SCRATCH/new_on_pre.txt"

diff "$SCRATCH/baseline.txt" "$SCRATCH/new_on_pre.txt" && echo AC3-PROVEN
```

Expected: `AC3-PROVEN` with no diff output. Any difference means the vocabulary change altered paths extraction and violates AC-3 — fix before continuing.

- [ ] **Step 10: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): extract branches and branches-ignore filter keys (SMA-540)" -m "One extractor now reads all four filter keys, so it is renamed to match.
Six existing fixtures gain branches records they previously ignored, and
one is renamed because its name asserted the opposite. Paths extraction
over the pre-rewrite workflow files is byte-identical, which is the
sound form of the AC-3 proof given this branch also reflows those files."
```

---

### Task 3: Add `branch_verdict`, its skip list, and the lazy `origin/main` canary

**Files:**
- Modify: `ci/actionlint/run.sh` — new definitions beside `pattern_verdict`; new `branch_filter_self_test`; wire it into the `--self-test` early exit and check 7

**Interfaces:**
- Consumes: `extract_filter_keys` from Task 2.
- Produces:
  - `branch_verdict <entry>` → exactly one of `ok | skipped | negated | unverifiable | invalid-name | unresolved | no-origin-main`
  - `origin_has <name>` → status 0 if `refs/remotes/origin/<name>` exists
  - `load_origin_refs` → populates `ORIGIN_REFS` (idempotent)
  - `origin_candidates` → space-separated sample of existing branch names, for messages
  - `no_origin_main_infra` → prints the canary message and exits 2 (main shell only)
  - `BRANCH_SKIP` array + `is_branch_skipped <entry>`

**Token vocabulary is disjoint from `pattern_verdict`'s on purpose** — see D8. Never reuse `dead`, `not-exact`, or a `rejected-*` token here.

- [ ] **Step 1: Write the failing fixtures**

Add this function immediately after `path_filter_self_test`:

```bash
# ---------------------------------------------------------------------------------------------
# Branch-filter self-test (definition only — invoked unconditionally as part of check 7).
#
# The standing control for check 5's branch half. It carries BOTH directions of the control pair:
# a name that must resolve and one that must not. A table whose verdicts all fire cannot tell a
# working check from a stuck one (SMA-466), and SMA-525's finding F4 was that a one-off mutation
# battery is not a standing control.
# ---------------------------------------------------------------------------------------------
branch_filter_self_test() {
  local rc=0 saved_skip

  # This table asserts a real ref resolves, so it shares the canary's precondition. Asserted here
  # rather than left to a confusing per-fixture mismatch: --self-test still needs no actionlint
  # binary, but it does now need a git repo carrying origin/main.
  load_origin_refs
  origin_has 'main' || no_origin_main_infra

  expect_branch() {
    local entry="$1" expected="$2" got
    got="$(branch_verdict "$entry")"
    if [ "$got" != "$expected" ]; then
      fail "branch-filter self-test: branch_verdict '$entry' returned '$got', expected
      '$expected'. Check 5's branch half is not deciding what it is documented to decide."
      rc=1
    fi
  }

  # The control pair. 'main' is asserted against the REAL ref store, so it also proves the lookup
  # is being consulted at all rather than short-circuiting to 'ok'.
  expect_branch 'main'                        'ok'
  expect_branch 'mian-sma540-absent'          'unresolved'

  # Glob metacharacters — rejected BEFORE check-ref-format so the message names the real reason.
  expect_branch 'release/**'                  'unverifiable'
  expect_branch 'v1.?'                        'unverifiable'
  expect_branch 'a+b'                         'unverifiable'
  expect_branch 'rel[0-9]'                    'unverifiable'

  # git's own ref-name rule, for everything the glob test lets through.
  expect_branch 'has space'                   'invalid-name'
  expect_branch 'a..b'                        'invalid-name'

  # Exclusions are never resolved (L3).
  expect_branch '!main'                       'negated'

  # The documented escape hatch actually silences an entry. BRANCH_SKIP ships empty, so this
  # overrides it for one assertion and restores it — the ${arr+"${arr[@]}"} idiom is required
  # because a bare "${arr[@]}" on an empty array is an unbound-variable error under `set -u`.
  saved_skip=(${BRANCH_SKIP+"${BRANCH_SKIP[@]}"})
  BRANCH_SKIP=('release/**')
  expect_branch 'release/**'                  'skipped'
  BRANCH_SKIP=(${saved_skip+"${saved_skip[@]}"})

  return $rc
}
```

Wire it in at both invocation points. In the `--self-test` early exit:

```bash
  '1:--self-test')
    # --self-test never shells out to actionlint, so it runs everything that does not need the
    # binary and exits before the PATH guard below. It DOES need a git repo carrying origin/main,
    # since branch_filter_self_test's control pair asserts a real ref resolves (SMA-540 D7).
    extractor_self_test
    path_filter_self_test
    branch_filter_self_test
    config_self_test
    exit "$FAILED" ;;
```

and in check 7:

```bash
extractor_self_test
path_filter_self_test
branch_filter_self_test
config_self_test
```

- [ ] **Step 2: Run the self-test to verify it fails**

Run: `ci/actionlint/run.sh --self-test`
Expected: FAIL — `load_origin_refs: command not found` / `branch_verdict: command not found`, and a non-zero exit.

- [ ] **Step 3: Write the implementation**

Add immediately after `pattern_verdict`'s closing brace:

```bash
# ---------------------------------------------------------------------------------------------
# Check 5, branch half (definitions) — every `branches:` entry must resolve as a ref, or be
# skip-listed (SMA-540 D2).
#
# `branches: [mian]` is a valid glob that actionlint accepts, so the workflow simply stops
# running — the same silent, permanent failure `paths:` had, one key over.
#
# The ref namespace is refs/remotes/origin/* ONLY (D3). A workflow triggers on branches as they
# exist on GitHub, which is exactly that set; refs/heads/* is a developer's private branch set
# locally and just `main` (push) or nothing (PR) in CI — the one namespace guaranteed to disagree.
# Measured: actions/checkout at fetch-depth 0 fetches +refs/heads/*:refs/remotes/origin/* on both
# the push and pull_request paths, so CI and a fetched local checkout see the same set.
#
# `branches-ignore:` is deliberately EXCLUDED from resolution (D6), mirroring paths-ignore: a
# typo'd exclusion makes the workflow run MORE often, which is the fail-safe direction.
# ---------------------------------------------------------------------------------------------
BRANCH_SKIP=(
  # (empty — add entries as "branch-or-pattern"  # why, and what verifies it instead)
)

is_branch_skipped() {
  local b="$1" s
  for s in ${BRANCH_SKIP+"${BRANCH_SKIP[@]}"}; do
    [ "$s" = "$b" ] && return 0
  done
  return 1
}

# The remote-tracking branch names, read ONCE. `refname:lstrip=3` drops `refs/remotes/origin/`, so
# a nested name (feature/x) survives intact where `refname:short` would prefix it with `origin/`.
# origin/HEAD is a symref to the default branch, not a branch, and is filtered out so it cannot
# make a literal entry named `HEAD` resolve.
#
# Loaded from the two MAIN-SHELL entry points below rather than lazily inside branch_verdict:
# verdicts are computed in nested command substitutions, so a cache populated there would be
# discarded with the subshell and re-run git once per entry.
ORIGIN_REFS=''
ORIGIN_REFS_LOADED=0

load_origin_refs() {
  [ "$ORIGIN_REFS_LOADED" -eq 1 ] && return 0
  ORIGIN_REFS="$(git for-each-ref --format='%(refname:lstrip=3)' refs/remotes/origin/ 2>/dev/null \
    | grep -vx 'HEAD')"
  ORIGIN_REFS_LOADED=1
  return 0
}

origin_has() {
  load_origin_refs
  printf '%s\n' "$ORIGIN_REFS" | grep -qxF -- "$1"
}

# A sample of what DOES exist, for the unresolved message. A bare "did not resolve" is the same
# unhelpful-message problem the canary exists to avoid, one level down.
origin_candidates() {
  load_origin_refs
  printf '%s\n' "$ORIGIN_REFS" | head -8 | tr '\n' ' '
}

# Exits 2. MAIN SHELL ONLY — called from the production call site and from
# branch_filter_self_test, never from inside a $( ), where it would exit only the subshell.
no_origin_main_infra() {
  infra "refs/remotes/origin/main does not resolve in this checkout, so no 'branches:' entry can
      be verified. This is an environment problem, not a workflow defect: run 'git fetch origin',
      or re-clone without --single-branch. If main was genuinely RENAMED, every branches: filter
      in this repo is now dead — update them and this canary together."
}

# The single source of truth for check 5's verdict on one branch entry. Echoes exactly one stable
# token; every non-'ok' token has a user-facing message at the production call site, and every
# token has a fixture in branch_filter_self_test. The vocabulary is deliberately DISJOINT from
# pattern_verdict's: PATTERN and BRANCH findings are separate record types and the call site
# dispatches on the record type, so a shared token would print the wrong message (D8).
branch_verdict() {
  local b="$1"

  is_branch_skipped "$b" && { echo 'skipped'; return; }

  # Exclusions are not resolved — requiring them to name a live branch would be wrong. They are
  # still COUNTED by check 6, so an all-negated block fails there with a specific verdict (L3).
  case "$b" in '!'*) echo 'negated'; return ;; esac

  # Glob metacharacters, tried FIRST and deliberately above check-ref-format (D4). GitHub reads
  # '*', '**', '?', '+' and '[]' as patterns, so the entry names a set rather than a branch and
  # cannot be resolved. Ordering is load-bearing twice over: check-ref-format would reject '*' and
  # '?' as illegal ref characters and report a true but useless reason; and it is what makes the
  # show-ref lookup below safe without pattern_verdict's explicit charset allowlist.
  #
  # '+' counts as a glob even though it is LEGAL in a git ref name: GitHub reads it as "one or
  # more of the preceding character", so 'foo+' matches the branch 'foo', and a branch literally
  # named 'foo+' would otherwise resolve and yield a confidently wrong 'ok'.
  case "$b" in
    *'*'*|*'?'*|*'+'*|*'['*|*']'*) echo 'unverifiable'; return ;;
  esac

  # git's own validity rule, so this gate does not enumerate one: it catches '..', '~', '^', ':',
  # control characters and a trailing '.lock'.
  if ! git check-ref-format "refs/heads/$b" 2>/dev/null; then
    echo 'invalid-name'; return
  fi

  # The canary is LAZY (D7) — only an entry that has survived every filter above actually needs a
  # ref, so a repo whose branch filters are all wildcards or skip-listed never pays it, and a
  # checkout without origin/main does not lose checks 1-6 as well. Returned as a TOKEN, not an
  # infra call: this function always runs inside $( ), where exit 2 would kill only the subshell.
  origin_has 'main' || { echo 'no-origin-main'; return; }

  origin_has "$b" && { echo 'ok'; return; }

  echo 'unresolved'
}
```

- [ ] **Step 4: Load the ref cache once in the main shell**

Immediately before the `for wf in "${WORKFLOW_FILES[@]}"` loop (checks 5/6 production call site), add:

```bash
# Populate the ref cache in the MAIN SHELL. Verdicts are computed in nested command
# substitutions, so a cache first populated there would be thrown away with the subshell and
# git would run once per entry instead of once per gate run.
load_origin_refs
```

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `ci/actionlint/run.sh --self-test && echo SELFTEST-GREEN`
Expected: `SELFTEST-GREEN`.

- [ ] **Step 6: Verify the full gate is still green**

Run: `ci/actionlint/run.sh && echo GATE-GREEN`
Expected: `GATE-GREEN`. Nothing dispatches to `branch_verdict` yet — that is Task 4.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): add branch_verdict and its origin ref lookup (SMA-540)" -m "A branches entry must resolve as refs/remotes/origin/<name> or sit in
BRANCH_SKIP with a justification. Glob metacharacters are rejected before
check-ref-format so the message names the real reason, and because that
ordering is what makes the lookup safe without a charset allowlist.

The origin/main canary is lazy and returns a token rather than calling
infra, since verdicts are computed inside command substitution where an
exit would kill only the subshell. Nothing dispatches to it yet."
```

---

### Task 4: Dispatch branch items in the scanner

**Files:**
- Modify: `ci/actionlint/run.sh` — `scan_workflow_records` (`flush_key` + the `ITEM` arm), `branch_filter_self_test` (scan fixtures)

**Interfaces:**
- Consumes: `branch_verdict` (Task 3), `extract_filter_keys` (Task 2).
- Produces: a new finding record `BRANCH\t<verdict>\t<entry>\t<lineno>`, consumed by Task 5's call site. `PATTERN`'s shape is unchanged (AC-3).

- [ ] **Step 1: Write the failing fixtures**

Add to `branch_filter_self_test`, immediately before its `return $rc`:

```bash
  expect_branch_scan() {
    local name="$1" expected="$2" records="$3" got
    got="$(scan_workflow_records "$records")"
    if [ "$got" != "$expected" ]; then
      fail "branch-filter self-test '$name' mismatch.
--- expected ---
$expected
--- actual ---
$got"
      rc=1
    fi
  }

  # Check 6, branch half. Without this fixture the all-negated widening has no standing control
  # and could be reverted silently — SMA-525 finding F4.
  expect_branch_scan 'an all-negated branches: block is a dead trigger' \
"$(printf 'KEY\tall-negated\tbranches\t5\t2')" \
"$(printf 'KEY\tbranches\t5\nITEM\tbranches\t!main\nITEM\tbranches\t!dev')"

  # The control that keeps the one above honest: a normal block must produce NOTHING. This is the
  # fixture that catches key_positive being left inside a paths-only guard, which would fire
  # all-negated on all five real filters.
  expect_branch_scan 'a branches block with a positive entry is clean' \
"" \
"$(printf 'KEY\tbranches\t5\nITEM\tbranches\tmain')"

  # Proves scan_workflow_records actually DISPATCHES branch items to branch_verdict, and that the
  # finding carries the key's line number so the message can say which filter.
  expect_branch_scan 'an unresolvable branch is reported with its key line' \
"$(printf 'BRANCH\tunresolved\tmian-sma540-absent\t5')" \
"$(printf 'KEY\tbranches\t5\nITEM\tbranches\tmain\nITEM\tbranches\tmian-sma540-absent')"

  # branches-ignore is counted but never resolved (D6) — a nonexistent branch here is a no-op.
  expect_branch_scan 'a branches-ignore entry is never resolved' \
"" \
"$(printf 'KEY\tbranches-ignore\t5\nITEM\tbranches-ignore\tmian-sma540-absent')"

  # ...but an unreadable branches-ignore is still a loud failure, not a silent skip.
  expect_branch_scan 'a branches-ignore KEY with no items is a failure' \
"$(printf 'KEY\tno-items\tbranches-ignore\t5\t0')" \
"$(printf 'KEY\tbranches-ignore\t5')"

  # End-to-end through the REAL extractor, not a hand-built records table — the only fixture that
  # proves the whole extractor -> scanner -> verdict pipeline reports a typo'd branch.
  tmp="$(mktemp)"
  printf '%s' 'name: t
on:
  push:
    branches:
      - mian-sma540-absent
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
' > "$tmp"
  expect_branch_scan 'a typod branch is reported end-to-end through the extractor' \
"$(printf 'BRANCH\tunresolved\tmian-sma540-absent\t4')" \
"$(extract_filter_keys "$tmp")"
  rm -f "$tmp"
```

Add `tmp` to the function's `local` line: `local rc=0 saved_skip tmp`.

- [ ] **Step 2: Run the self-test to verify it fails**

Run: `ci/actionlint/run.sh --self-test`
Expected: FAIL. `'an all-negated branches: block is a dead trigger'` reports an empty `--- actual ---`, and `'an unresolvable branch is reported with its key line'` likewise — nothing dispatches branches yet.

- [ ] **Step 3: Widen `flush_key`**

Replace:

```bash
    elif [ "$key_kind" = 'paths' ] && [ "$key_items" -gt 0 ] && [ "$key_positive" -eq 0 ]; then
```

with:

```bash
    elif { [ "$key_kind" = 'paths' ] || [ "$key_kind" = 'branches' ]; } \
      && [ "$key_items" -gt 0 ] && [ "$key_positive" -eq 0 ]; then
```

- [ ] **Step 4: Move the positive count out of the paths-only guard and dispatch on kind**

Replace the whole `ITEM)` arm:

```bash
      ITEM)
        key_items=$((key_items + 1))
        if [ "$kind" = 'paths' ]; then
          verdict="$(pattern_verdict "$value")"
          case "$verdict" in
            ok|skipped|negated) ;;
            *) printf 'PATTERN\t%s\t%s\n' "$verdict" "$value" ;;
          esac
          case "$value" in '!'*) ;; *) key_positive=$((key_positive + 1)) ;; esac
        fi ;;
```

with:

```bash
      ITEM)
        key_items=$((key_items + 1))
        # COUNTING IS KIND-GENERIC; only the verdict dispatch below is kind-specific. This
        # increment used to live inside the paths-only guard, where a branches: block would
        # report key_positive=0 and fire 'all-negated' on all five real filters — redding the
        # only required check on a clean tree (SMA-540 D5).
        case "$value" in '!'*) ;; *) key_positive=$((key_positive + 1)) ;; esac
        case "$kind" in
          paths)
            verdict="$(pattern_verdict "$value")"
            case "$verdict" in
              ok|skipped|negated) ;;
              *) printf 'PATTERN\t%s\t%s\n' "$verdict" "$value" ;;
            esac ;;
          branches)
            # A separate record type, carrying the key's line number: PATTERN's shape stays
            # byte-identical for AC-3, the two verdict vocabularies cannot collide into the wrong
            # message, and a message can name WHICH of two identical `branches:` entries is wrong.
            verdict="$(branch_verdict "$value")"
            case "$verdict" in
              ok|skipped|negated) ;;
              *) printf 'BRANCH\t%s\t%s\t%s\n' "$verdict" "$value" "$key_line" ;;
            esac ;;
        esac ;;
```

- [ ] **Step 5: Run the self-test to verify it passes**

Run: `ci/actionlint/run.sh --self-test && echo SELFTEST-GREEN`
Expected: `SELFTEST-GREEN`.

- [ ] **Step 6: Verify the full gate is green on the real tree**

Run: `ci/actionlint/run.sh && echo GATE-GREEN`
Expected: `GATE-GREEN`. This is the run that would have failed had `key_positive` been left in the paths-only guard.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): dispatch branch entries through the record scanner (SMA-540)" -m "Branch items now get a verdict and their own BRANCH finding record, which
carries the key line so a message can name which of two identical filters
is wrong, and keeps the PATTERN record shape untouched for AC-3.

The positive-entry count moves out of its paths-only guard. Widening only
the all-negated condition, as the design first described, would have made
every branches block report zero positive entries and fired all-negated on
all five real filters."
```

---

### Task 5: Messages at the production call site

**Files:**
- Modify: `ci/actionlint/run.sh` — the `while IFS=$'\t' read -r rec verdict f1 f2 f3` loop in checks 5/6

**Interfaces:**
- Consumes: `BRANCH` records (Task 4), `no_origin_main_infra` / `origin_candidates` (Task 3).
- Produces: user-facing failures. No new records.

- [ ] **Step 1: Write the failing end-to-end test**

Create a temporary workflow carrying a typo'd branch. It goes in `.github/workflows/` because that is what `WORKFLOW_FILES` globs, and it must be otherwise valid so check 1 stays clean:

```bash
cat > .github/workflows/zz-sma540-probe.yml <<'YAML'
name: zz-sma540-probe
on:
  push:
    branches:
      - mian
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
YAML
ci/actionlint/run.sh; echo "exit=$?"
```

Expected at this point: **`exit=0`** — the gate passes despite the typo. The outer `case "$rec" in` has only `PATTERN)` and `KEY)` arms and **no catch-all**, so a `BRANCH` record is silently dropped. That silent pass is exactly the failure this task closes, and it is why Step 2 adds a catch-all as well as the new arm: an unhandled record type must never read as "nothing to report".

- [ ] **Step 2: Add the `BRANCH` arm and a catch-all**

Inside the `case "$rec" in` block, between the `PATTERN)` and `KEY)` arms:

```bash
      BRANCH)
        b="$f1"; bline="$f2"
        case "$verdict" in
          unverifiable)
            fail "$wf:$bline: branches entry '$b' contains a glob metacharacter ('*', '?', '+' or
      '[]'), so it names a pattern rather than a branch and cannot be resolved against a ref.
      Rewrite it as a literal branch name, or add it to BRANCH_SKIP in $0 with a justification
      saying what verifies it instead." ;;
          invalid-name)
            fail "$wf:$bline: branches entry '$b' is not a legal git branch name — git
      check-ref-format rejects it. No branch can ever carry that name, so the trigger it guards is
      dead." ;;
          unresolved)
            fail "$wf:$bline: branches entry '$b' does not resolve as refs/remotes/origin/$b. The
      trigger it guards is (or will become) dead — GitHub reports nothing when a branch filter
      matches nothing. Existing branches include: $(origin_candidates). If the branch does not
      exist yet, add '$b' to BRANCH_SKIP in $0 with a justification." ;;
          no-origin-main)
            no_origin_main_infra ;;
          *)
            infra "unhandled branch verdict '$verdict' for '$b' in $wf" ;;
        esac ;;
```

Then close the silent-drop hole by adding a catch-all as the LAST arm of the outer `case "$rec" in`, after `KEY) … ;;`:

```bash
      *)
        # A finding record whose type nothing handles must not read as "nothing to report" —
        # that is the silent pass this whole gate exists to prevent, one layer up. Before
        # SMA-540 added BRANCH, an unknown record type fell out of this case with no action.
        infra "unhandled finding record type '$rec' in $wf" ;;
```

- [ ] **Step 3: Run the end-to-end test to verify it now names the file, line and entry**

Run: `ci/actionlint/run.sh; echo "exit=$?"`
Expected: `exit=1`, and a message containing `zz-sma540-probe.yml:4:`, `'mian'`, and `Existing branches include:` followed by real names including `main`.

- [ ] **Step 4: De-hardcode the two check-6 messages**

Both name `paths:` regardless of the kind in the record. Replace the `no-items` message:

```bash
          no-items)
            fail "$wf:$f2: '$f1:' has no sequence entries this gate could read. Two forms produce
      that and neither is parsed: an inline sequence ($f1: [a, b]) and a flow mapping on the
      event itself (push: { $f1: [a, b] }). Rewrite the event and its filter in block style —
      skipping either one silently is exactly the failure this gate exists to prevent." ;;
```

and the `all-negated` message:

```bash
          all-negated)
            fail "$wf:$f2: '$f1:' has $f3 entries but every one is a '!'-negated exclusion. GitHub
      requires at least one non-'!' entry — a filter made only of exclusions can never match, so
      the trigger it guards is dead. Add at least one positive entry." ;;
```

- [ ] **Step 5: Prove the de-hardcoded message reports the right key**

```bash
cat > .github/workflows/zz-sma540-probe.yml <<'YAML'
name: zz-sma540-probe
on:
  push:
    branches:
      - '!main'
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
YAML
ci/actionlint/run.sh 2>&1 | grep -F "'branches:'"
```

Expected: a line containing `'branches:' has 1 entries but every one is a '!'-negated exclusion`. Before this step it would have said `'paths:'` at a line where no `paths:` key exists.

- [ ] **Step 6: Remove the probe and confirm the tree is clean**

```bash
rm -f .github/workflows/zz-sma540-probe.yml
git status --short
ci/actionlint/run.sh && echo GATE-GREEN
```

Expected: `git status --short` shows only `ci/actionlint/run.sh` as modified — **no** `zz-sma540-probe.yml`. Then `GATE-GREEN`. Committing that probe would put a broken workflow in `.github/workflows/` where GitHub itself parses it.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/run.sh
git commit -m "feat(repo): report branch filter findings with file, line and remedy (SMA-540)" -m "Each branch verdict gets a message naming what to do about it, and the
unresolved case lists branches that do exist rather than reporting a bare
boolean.

The two check-6 messages said 'paths:' regardless of which key the record
carried, so an all-negated branches block was reported as a paths problem
at a line where no paths key exists. They now read the kind out of the
record, which the block-style rule also depends on for its remedy."
```

---

### Task 6: Prove the new checks bite (mutation battery)

**Files:** none — this task makes no commit unless it finds a gap.

**Interfaces:**
- Consumes: everything from Tasks 2–5.
- Produces: evidence for the PR body. If any mutation stays green, STOP and fix the gap before Task 7.

SMA-525's finding F4 was that checks 5/6 could be neutered one code path at a time with the gate still exiting 0, because the only thing exercising them was the repo's own clean files. Each mutation below must red.

**Mutation hygiene:** revert with `git checkout -- ci/actionlint/run.sh` after each one, never by hand-editing back — and never with a `.bak` copy, because `mv` rolls mtime backwards and can make a later run reuse stale state.

- [ ] **Step 1: Mutation A — the resolve lookup is neutered**

Change `origin_has "$b" && { echo 'ok'; return; }` to `echo 'ok'; return`.
Run: `ci/actionlint/run.sh --self-test; echo "exit=$?"`
Expected: `exit=1`, naming `branch_verdict 'mian-sma540-absent' returned 'ok', expected 'unresolved'`.
Revert: `git checkout -- ci/actionlint/run.sh`

- [ ] **Step 2: Mutation B — the control pair's positive direction is neutered**

Change `origin_has "$b" && { echo 'ok'; return; }` to `origin_has "$b" && { echo 'unresolved'; return; }`.
Run: `ci/actionlint/run.sh --self-test; echo "exit=$?"`
Expected: `exit=1`, naming `branch_verdict 'main' returned 'unresolved', expected 'ok'`. This is what proves the table is not merely all-firing.
Revert: `git checkout -- ci/actionlint/run.sh`

- [ ] **Step 3: Mutation C — branches dropped from the extractor vocabulary**

Change the block-form key regex to `/^["\047]?(paths)(-ignore)?["\047]?[ \t]*:/`.
Run: `ci/actionlint/run.sh --self-test; echo "exit=$?"`
Expected: `exit=1`, naming `extractor self-test 'a branches block is extracted' mismatch`.
Revert: `git checkout -- ci/actionlint/run.sh`

- [ ] **Step 4: Mutation D — the all-negated widening reverted**

Change `flush_key`'s condition back to `[ "$key_kind" = 'paths' ]`.
Run: `ci/actionlint/run.sh --self-test; echo "exit=$?"`
Expected: `exit=1`, naming `'an all-negated branches: block is a dead trigger' mismatch`.
Revert: `git checkout -- ci/actionlint/run.sh`

- [ ] **Step 5: Mutation E — key_positive moved back into the paths-only guard**

Wrap the `key_positive` increment in `if [ "$kind" = 'paths' ]; then … fi`.
Run: `ci/actionlint/run.sh; echo "exit=$?"`
Expected: `exit=1` on the REAL tree, with `all-negated` fired against `ci.yml`, `prebuild.yml` and `security-scan.yml`. This reproduces the clean-tree red the reviewer caught in the design, and proves Task 4 Step 4 is load-bearing.
Revert: `git checkout -- ci/actionlint/run.sh`

- [ ] **Step 6: Mutation F — the scanner stops dispatching branches**

Delete the `branches)` arm from the `ITEM)` case.
Run: `ci/actionlint/run.sh --self-test; echo "exit=$?"`
Expected: `exit=1`, naming `'an unresolvable branch is reported with its key line' mismatch` and `'a typod branch is reported end-to-end through the extractor' mismatch`.
Revert: `git checkout -- ci/actionlint/run.sh`

- [ ] **Step 7: Confirm the tree is back to green and unmodified**

```bash
git status --short
ci/actionlint/run.sh && echo GATE-GREEN
```

Expected: no modifications to `ci/actionlint/run.sh`, then `GATE-GREEN`. If any mutation above stayed green, that code path has no standing control — add a fixture for it and re-run the whole battery.

---

### Task 7: Documentation

**Files:**
- Modify: `ci/actionlint/README.md`
- Modify: `moon.yml` — the `repo:actionlint` `description:`
- Modify: `CLAUDE.md` — one gotcha

- [ ] **Step 1: Update the README checks table**

No row is renumbered (D10). Reword rows 5, 6 and 7:

```markdown
| 5 | Every `paths:` glob is in the supported vocabulary and matches the tracked tree, and every `branches:` entry resolves as a ref or is skip-listed |
| 6 | Every extracted filter key carries at least one sequence entry, at least one of them positive |
| 7 | Four self-tests against fixture tables — extractor, path-filter verdicts, branch-filter verdicts, config allowlist (`run.sh --self-test`) |
```

- [ ] **Step 2: Update the README subtitle and intro**

Line 3 currently reads "Lints `.github/workflows/**` and proves every `paths:` filter glob still matches the tree." Replace with:

```markdown
Lints `.github/workflows/**`, proves every `paths:` filter glob still matches the tree, and
proves every `branches:` filter entry names a branch that exists.
```

In the "Why" section, after the existing `paths:` paragraph, add:

```markdown
`branches:` has the identical property and was SMA-525's stated limitation L5. `branches: [mian]`
is a valid glob, actionlint accepts it, and the workflow stops running — silently and permanently,
one key over. All three workflows here trigger off `branches: [main]`, including the required
check. See SMA-540 and
`docs/superpowers/specs/2026-08-19-sma-540-branches-filter-gate-design.md`.
```

- [ ] **Step 3: Add the branch vocabulary and escape hatch to the README**

After the "Supported glob vocabulary" section, add:

```markdown
## Branch filter entries

`branches:` is read as a **block sequence** — the inline `branches: [main]` form is deliberately
not parsed and fails check 6 by design, exactly as `paths: [a, b]` does. Each entry must:

- **resolve** as `refs/remotes/origin/<name>`, or
- appear in `BRANCH_SKIP` in `run.sh` with a comment justifying it.

Local `refs/heads/*` is deliberately **not** consulted: a workflow triggers on branches as they
exist on GitHub, and a local-only branch does not. A glob metacharacter (`*`, `**`, `?`, `+`,
`[]`) makes an entry a pattern rather than a name, so it cannot be resolved and must be
skip-listed — `+` counts as a glob even though git allows it in a ref name, because GitHub reads
it as "one or more of the preceding character".

`branches-ignore:` is extracted and counted but never resolved: a typo'd exclusion makes a
workflow run *more* often, which is the fail-safe direction.

`tags:` and `tags-ignore:` are not covered — see the spec's §7 L4.
```

In "Escape hatches", after the `SKIP_PATTERNS` bullet, add:

```markdown
- A **branch that does not exist yet**, or a branch pattern: add it to `BRANCH_SKIP` in `run.sh`
  with a comment justifying it and saying what verifies it instead.
```

- [ ] **Step 4: Correct the stale rollback line in the README**

Line 57 currently claims dropping `:actionlint` is "One line". Replace that bullet with:

```markdown
- **Anything worse**: drop `:actionlint` from `T=(…)` in `.github/workflows/ci.yml`. Once SMA-541
  lands this must be removed from the CLAUDE.md `ci-targets` block as well, since a gate asserts
  the two agree.
```

- [ ] **Step 5: Update the Moon task description**

In `moon.yml`, the `repo:actionlint` task `description:` currently says "plus a control that every paths: filter glob still matches the tree (SMA-525)". Replace with:

```yaml
    description: 'actionlint over .github/workflows/**, plus controls that every paths: glob still matches the tree and every branches: entry names a real branch (SMA-525, SMA-540).'
```

- [ ] **Step 6: Add the CLAUDE.md gotcha**

Add to the Gotchas section. **Prose only** — no `ci-targets` marker strings, no pasted `moon ci` command (spec §6):

```markdown
- Workflow trigger filters are gated by `repo:actionlint`. Write `branches:` and `paths:` as
  **block sequences**, never the inline `branches: [main]` form — the gate's extractor does not
  parse inline flow and fails it loudly rather than skipping it in silence. Every wildcard-free
  `branches:` entry must resolve as `refs/remotes/origin/<name>`; a branch that does not exist yet,
  or any entry carrying a glob character (`*`, `?`, `+`, `[]` — `+` included, since GitHub reads it
  as a quantifier), needs a justified `BRANCH_SKIP` entry in `ci/actionlint/run.sh`. A typo'd
  branch name otherwise disables a workflow silently and permanently (SMA-540).
```

- [ ] **Step 7: Verify the gate and the docs agree**

Run (the PATH export is required — both `actionlint` and `moon` are proto-managed and are not on a
non-interactive shell's default PATH; without it you may silently get a system `actionlint`):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/actionlint/run.sh && echo GATE-GREEN
moon run repo:actionlint
```

Expected: `GATE-GREEN`, then a passing Moon task. `moon.yml` is an input to nothing here, but a YAML typo in it would break the next Moon invocation, which is why the second command is run at all.

- [ ] **Step 8: Commit**

```bash
git add ci/actionlint/README.md moon.yml CLAUDE.md
git commit -m "docs(repo): document the branch filter gate and its escape hatch (SMA-540)" -m "Records the block-sequence requirement, the resolve-or-skip-list rule and
why a local ref does not count. Also corrects the README's claim that
dropping the gate from the CI target list is a one-line change, which
stops being true once SMA-541 lands its two-way agreement check."
```

---

### Task 8: Full-graph verification

**Files:** none.

Per CLAUDE.md, per-project Moon tasks do not run the repo-level gates. Run the graph the way CI does before pushing.

- [ ] **Step 1: Run the full affected graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  --base origin/main --include-relations
```

Expected: all tasks pass. If Moon reports an unattributed failure, diagnose with
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`.

- [ ] **Step 2: Confirm the working tree carries only the intended files**

```bash
git status --short
git diff --stat origin/main
```

Expected: a clean tree, and a diff touching exactly: the three workflows, `ci/actionlint/run.sh`, `ci/actionlint/README.md`, `moon.yml`, `CLAUDE.md`, and the two `docs/superpowers/` files. Anything else — especially a `zz-sma540-probe.yml` or a scratch script — must be removed.

- [ ] **Step 3: Confirm `ci.yml`'s `T=(…)` array is untouched**

```bash
git diff origin/main -- .github/workflows/ci.yml
```

Expected: the diff shows **only** the two `branches:` hunks. The `T=(…)` array must not appear — this PR adds no new gate, so `repo:affected-smoke` has nothing to re-baseline, and touching `T` would collide with SMA-541.

---

## Post-merge (not part of the PR)

`ci.yml`'s `push.branches` filter is read from the merge commit, not the PR head, so a mistake in it is invisible on the PR and would silently stop main's post-merge CI and cache warming — the SMA-448 shape, applied to the required check's own trigger. After merging, confirm a `CI / moon ci` run appeared on `main`:

```bash
gh run list --workflow=ci.yml --event=push --limit 3
```

If none appeared, revert the `ci.yml` hunk immediately.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
