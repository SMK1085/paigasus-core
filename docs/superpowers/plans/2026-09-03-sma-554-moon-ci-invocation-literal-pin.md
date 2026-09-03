# SMA-554 — `moon ci` invocation literal pin: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `check_invocation`'s pattern-matched shape rule in `ci/affected-graph/ci_targets.py` with a positionally-anchored exact-literal pin of `ci.yml`'s eight-line `moon ci` branch block, keeping the existing regex only as an extras counter.

**Architecture:** Two independent assertions over `ci.yml`'s text. **A** requires `MOON_CI_BRANCH_BLOCK`'s eight lines to appear as a consecutive, byte-identical run beginning on the line immediately after the sole `T=(…)` line. **B** counts command-position `moon ci` lines and requires exactly `EXPECTED_MOON_CI_INVOCATIONS` (2). A module-scope invariant asserts the two constants agree. `T_ARRAY_EXPANSION`, `_strip_comment` and `MOON_CI_INVOCATION` are deleted.

**Tech Stack:** Python 3 stdlib only (`re`, `difflib`), no new dependencies. Gate runs under `repo:affected-smoke` via `ci/affected-graph/run.sh`. Linted by `repo:ruff-ci` against `py/pyproject.toml`.

**Spec:** `docs/superpowers/specs/2026-09-03-sma-554-moon-ci-invocation-literal-pin-design.md`

## Global Constraints

- Every source file opens with an SPDX header (`# SPDX-License-Identifier: Apache-2.0` for Python). `ci_targets.py` already has one — do not add a second.
- Bash tool PATH lacks proto-managed CLIs. Prefix every `moon`/`uv` command with:
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- Conventional commits with a workspace scope. This work is `ci(repo): …` or `docs(repo): …`.
- **Commitlint trap:** a wrapped body line beginning `word:` is parsed as a footer and reds `footer-leading-blank`. Never start a body line with a word followed by a colon.
- `ci/**/*.py` must pass `repo:ruff-ci` (rule set from `py/pyproject.toml`, `line-length = 200`). Do **not** run `ruff format` over `ci/` — it is deliberately not gated there.
- **`MOON_CI_BRANCH_BLOCK`'s entries are copied VERBATIM from `.github/workflows/ci.yml`, indentation included.** Never hand-format an entry. Same rule `T_INVOCATION_ALLOWLIST` states for itself.
- The eight pinned lines have **three other co-update sites** (spec §6). Do not edit `.github/workflows/ci.yml`'s block in this work — it stays exactly as it is.
- `MOON_CI_LINE_RE`, `EXPECTED_MOON_CI_INVOCATIONS`, `T_ARRAY_RE`, `T_ASSIGN_RE` and `parse_t` keep their current names and semantics unless a task says otherwise.

---

## File structure

| File | Responsibility | Change |
| -- | -- | -- |
| `ci/affected-graph/ci_targets.py` | the gate | constants (Task 1), `check_invocation` + `_block_anchor` (Task 2), fix message (Task 3), fixtures (Task 4), stale `T_ARRAY_RE` comment (Task 3) |
| `ci/affected-graph/README.md` | gate documentation | C5's entry rewritten (Task 5) |
| `docs/superpowers/specs/2026-08-19-sma-541-…-design.md` | prior art | C5 entry + fixture rows annotated superseded; L10 annotated **still open** (Task 5) |

All production code lives in one file because that is this gate's established structure — `ci_targets.py` holds every check, its constants and its self-test together. Splitting one check out would break the `--self-test` battery's single entry point.

---

### Task 1: The constants

**Files:**
- Modify: `ci/affected-graph/ci_targets.py:75-118` (the `MOON_CI_*` constant block)

**Interfaces:**
- Consumes: nothing.
- Produces: `MOON_CI_BRANCH_BLOCK: tuple[str, ...]` (eight strings), and the surviving
  `MOON_CI_LINE_RE: re.Pattern` / `EXPECTED_MOON_CI_INVOCATIONS: int = 2`.
  Deletes `MOON_CI_INVOCATION` and `T_ARRAY_EXPANSION`.

- [ ] **Step 1: Confirm the eight lines byte-for-byte before copying them**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
awk 'NR>=235 && NR<=242 {printf "[%s]\n", $0}' .github/workflows/ci.yml
```

Expected — exactly these eight, with the brackets showing leading whitespace and no trailing space:
```
[          if [ "$EVENT" = "pull_request" ]; then]
[            moon ci "${T[@]}" --base origin/main --include-relations]
[          elif [ -n "${BEFORE:-}" ] && ! printf '%s' "$BEFORE" | grep -qE '^0+$'; then]
[            moon ci "${T[@]}" --base "$BEFORE" --include-relations]
[          else]
[            # Initial push with no usable base — run the whole graph to warm caches.]
[            moon run "${T[@]}"]
[          fi]
```

If the output differs, **stop** — `ci.yml` has moved since the spec was written and the whole plan needs re-basing.

- [ ] **Step 2: Delete `MOON_CI_INVOCATION` and `T_ARRAY_EXPANSION`**

Delete `ci_targets.py:75-81` — the `MOON_CI_INVOCATION = 'moon ci "${T[@]}"'` assignment together with its comment block, and the `T_ARRAY_EXPANSION = '"${T[@]}"'` assignment together with its comment block. Leave `T_ARRAY_RE` (`:67`) and `T_ASSIGN_RE` (`:58`) untouched.

- [ ] **Step 3: Add `MOON_CI_BRANCH_BLOCK` in their place**

Insert immediately above the surviving `MOON_CI_LINE_RE` definition:

```python
# The `moon ci` step's whole branch block, pinned as an EXACT LITERAL rather than matched by
# shape. SMA-541's regex-based predecessor was bypassed four times during its own review — a
# subsetted array, a subset behind a leading flag, an `echo` prefix, multiple spaces, and a
# trailing comment supplying the expansion — and every one was "the pattern did not describe the
# thing someone wrote". There is no clever spelling of a string that is not that string.
#
# Copied VERBATIM from .github/workflows/ci.yml, indentation included — re-verify against the real
# file (`awk 'NR>=235 && NR<=242' .github/workflows/ci.yml`) before editing this tuple; do not
# hand-format an entry. Same rule ci/actionlint/run.sh's T_INVOCATION_ALLOWLIST states for itself,
# and keeping both pins on that one rule is what makes co-updating them mechanical.
#
# CO-UPDATE SITES (SMA-554 §6). An edit to these lines touches FOUR places, not one:
#   1. .github/workflows/ci.yml:235-242                      — the block itself
#   2. ci/actionlint/run.sh, T_INVOCATION_ALLOWLIST          — check 8b's exact-literal pin
#   3. ci/actionlint/run.sh, block_execution_verdict         — check 8d's derived expectation
#   4. this tuple
# Checks 8b and 8d are NOT redundant with this one and must not be deleted on the grounds that
# ci_targets.py now pins the same lines: 8d EXECUTES the block against a stubbed `moon` on four
# GitHub event paths, which is the only control that sees a step-level `if: false` or an
# `if false; then … fi` wrap (L2). This tuple is a second, independently-scheduled opinion.
#
# The `T=(…)` line directly above the block is deliberately NOT part of this tuple: every new
# `repo:*` gate appends to it, so including it would red this gate on the single most routine edit
# in the repo, and C1-C3 already assert the array's contents. It is used as the ANCHOR instead.
MOON_CI_BRANCH_BLOCK = (
    '          if [ "$EVENT" = "pull_request" ]; then',
    '            moon ci "${T[@]}" --base origin/main --include-relations',
    "          elif [ -n \"${BEFORE:-}\" ] && ! printf '%s' \"$BEFORE\" | grep -qE '^0+$'; then",
    '            moon ci "${T[@]}" --base "$BEFORE" --include-relations',
    "          else",
    "            # Initial push with no usable base — run the whole graph to warm caches.",
    '            moon run "${T[@]}"',
    "          fi",
)
```

- [ ] **Step 4: Rewrite `MOON_CI_LINE_RE`'s comment to record its demotion**

Replace the entire comment block above `MOON_CI_LINE_RE` (currently `ci_targets.py:83-110`, the "Which lines count as an invocation…" essay) with:

```python
# DEMOTED (SMA-554): this regex no longer defines any SHAPE rule — MOON_CI_BRANCH_BLOCK above does,
# by exact literal. Its only remaining job is CARDINALITY: count command-position `moon ci` lines
# and require exactly EXPECTED_MOON_CI_INVOCATIONS, which is the one thing an exact-literal pin
# cannot see (a THIRD invocation added elsewhere in the file, or a duplicate of the block).
#
# That demotion is what makes its known false negatives tolerable rather than holes. `$MOON ci …`,
# a `FOO=1 moon ci …` env-assignment prefix, a `moon()` shell-function shadow and a `\`-continued
# invocation are all uncounted (L1) — but check 8d in ci/actionlint/run.sh catches every one of
# them behaviourally, by executing the block and counting real `moon` invocations.
#
# Anchored at COMMAND POSITION — `moon` must be the line's first token — with `[ \t]+` between the
# two words, so neither a `#` comment nor a `name:` field is mistaken for an invocation. Verified
# against the real ci.yml: it matches exactly the two invocation lines, and none of the eight prose
# comments or two `name:` fields that mention `moon ci`.
MOON_CI_LINE_RE = re.compile(r"^[ \t]*moon[ \t]+ci\b.*$", re.MULTILINE)
```

- [ ] **Step 5: Rewrite `EXPECTED_MOON_CI_INVOCATIONS`' comment and add the D8 invariant**

Replace the comment above `EXPECTED_MOON_CI_INVOCATIONS` (currently `:112-117`) and append the invariant directly below the assignment:

```python
# How many command-position `moon ci` lines the whole file may carry. A genuinely new third
# invocation reds and must be reviewed — the same default-deny stance D10 takes on a
# project-scoped `T` entry.
EXPECTED_MOON_CI_INVOCATIONS = 2

# The two constants above must agree, and nothing else asserts it (SMA-554 D8). B's "exactly 2 in
# the file" only means "the block's two and none outside it" because MOON_CI_BRANCH_BLOCK happens
# to contribute exactly two matching lines. A future edit changing the block's invocation count
# while updating only one constant would leave B silently meaning something else — the same
# two-derived-things-drift-apart class the floors elsewhere in this file guard against. Asserted at
# MODULE SCOPE so it fires on import, before any check runs, rather than only under --self-test.
_BLOCK_INVOCATIONS = sum(1 for line in MOON_CI_BRANCH_BLOCK if MOON_CI_LINE_RE.match(line))
assert _BLOCK_INVOCATIONS == EXPECTED_MOON_CI_INVOCATIONS, (
    f"MOON_CI_BRANCH_BLOCK carries {_BLOCK_INVOCATIONS} `moon ci` line(s) but "
    f"EXPECTED_MOON_CI_INVOCATIONS is {EXPECTED_MOON_CI_INVOCATIONS}; update both together"
)
```

- [ ] **Step 6: Verify the module still imports and the invariant holds**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
import ci_targets as m
print('block lines:', len(m.MOON_CI_BRANCH_BLOCK))
print('invocations:', m._BLOCK_INVOCATIONS)
print('deleted ok:', not hasattr(m, 'MOON_CI_INVOCATION'), not hasattr(m, 'T_ARRAY_EXPANSION'))
"
```
Expected:
```
block lines: 8
invocations: 2
deleted ok: True True
```

- [ ] **Step 7: Verify the constant matches the real file exactly**

This is the assertion that catches a hand-formatted entry. Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 -c "
import sys, pathlib; sys.path.insert(0, 'ci/affected-graph')
import ci_targets as m
lines = pathlib.Path('.github/workflows/ci.yml').read_text().split('\n')
want = list(m.MOON_CI_BRANCH_BLOCK)
print('found at anchor:', lines[234:242] == want)
for a, b in zip(want, lines[234:242]):
    if a != b: print('DIFF\n  const:', repr(a), '\n  file :', repr(b))
"
```
Expected: `found at anchor: True` and no `DIFF` lines.

- [ ] **Step 8: Commit**

```bash
cd /Users/sven/dev/paigasus/paigasus-core
git add ci/affected-graph/ci_targets.py
git commit -m "ci(repo): pin ci.yml's moon ci branch block as an exact literal (SMA-554)

Adds MOON_CI_BRANCH_BLOCK, copied verbatim from ci.yml, and deletes
MOON_CI_INVOCATION and T_ARRAY_EXPANSION. Demotes MOON_CI_LINE_RE to a
cardinality counter and asserts at module scope that the two constants
agree, which nothing did before.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ"
```

Note the check itself does not use the new constant yet — that is Task 2. The tree stays green because `check_invocation` is untouched here.

---

### Task 2: The anchored block check

**Files:**
- Modify: `ci/affected-graph/ci_targets.py:1312-1327` (delete `_strip_comment`)
- Modify: `ci/affected-graph/ci_targets.py:1329-1366` (rewrite `check_invocation`)

**Interfaces:**
- Consumes: `MOON_CI_BRANCH_BLOCK`, `MOON_CI_LINE_RE`, `EXPECTED_MOON_CI_INVOCATIONS` (Task 1); `T_ARRAY_RE` (existing).
- Produces: `check_invocation(ci_yml_text: str) -> list[str]` — unchanged signature and unchanged contract (empty list = clean), so `main()` at `:2958` needs no edit. Also `_block_anchor(lines: list[str]) -> int | None`.

- [ ] **Step 1: Add the failing fixture first**

This gate's tests live in the `--self-test` battery, not a pytest file. Add this **temporary** probe at the very top of the invocation fixture region (`ci_targets.py:1883`, just before the existing `invoked = (` assignment) so there is a red to drive the implementation:

```python
    # TASK 2 PROBE — replaced wholesale by Task 4's fixture battery.
    _canonical = (
        "        run: |\n"
        "          set -euo pipefail\n"
        "          T=(:build :test)\n"
        + "\n".join(MOON_CI_BRANCH_BLOCK) + "\n"
    )
    if check_invocation(_canonical):
        failures.append(
            f"check_invocation: fired on the canonical block: {check_invocation(_canonical)}"
        )
    _moved = _canonical.replace(
        "          T=(:build :test)\n", "          T=(:build :test)\n          echo hello\n"
    )
    if not check_invocation(_moved):
        failures.append("check_invocation: missed a block detached from its T= anchor")
```

- [ ] **Step 2: Run the self-test and verify the probe fails**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 ci/affected-graph/ci_targets.py --self-test 2>&1 | tail -20
```
Expected: FAIL, naming `check_invocation: missed a block detached from its T= anchor` — the current implementation has no anchor concept, so a detached block passes.

- [ ] **Step 3: Delete `_strip_comment`**

Delete `ci_targets.py:1312-1327` entirely — the whole `def _strip_comment(line):` function and its docstring. Nothing else references it (spec E5).

- [ ] **Step 4: Add `_block_anchor` above `check_invocation`**

```python
def _block_anchor(lines):
    """Index of the line where MOON_CI_BRANCH_BLOCK must begin, or None if there is no anchor.

    The anchor is the line immediately AFTER the sole `T=(…)` array line. `parse_t` already
    rejects a file carrying anything other than exactly one `T=(…)` array and exactly one
    `T=`/`T+=` assignment, and it runs first inside main()'s try block — so by the time this is
    reached in production the uniqueness holds. It is re-derived here rather than assumed, because
    check_invocation is also called directly by --self-test on synthetic texts that never went
    through parse_t.

    Anchoring is what closes the DECOY family: without it, A searches the whole file, so a verbatim
    copy of the eight lines pasted into an unrelated step satisfies the pin while the real step is
    rewritten into forms MOON_CI_LINE_RE does not count. A decoy cannot bring its own `T=` anchor
    along — a second one makes parse_t red instead (SMA-554 D7/E7).

    Returns None when the anchor is absent or ambiguous, which check_invocation reports as its own
    row rather than silently passing.
    """
    hits = [i for i, line in enumerate(lines) if T_ARRAY_RE.match(line)]
    if len(hits) != 1:
        return None
    return hits[0] + 1
```

- [ ] **Step 5: Rewrite `check_invocation`**

Replace the whole function (docstring included) with:

```python
def check_invocation(ci_yml_text):
    """`ci.yml`'s `moon ci` branch block, pinned as an exact literal at a fixed anchor.

    Two independent assertions, both returned as rows:

      A — the eight lines of MOON_CI_BRANCH_BLOCK appear as a consecutive, in-order,
          byte-identical run beginning immediately after the sole `T=(…)` line.
      B — the whole file carries exactly EXPECTED_MOON_CI_INVOCATIONS command-position
          `moon ci` lines.

    B's bound is only meaningful because A's block contributes exactly two matching lines; the
    module-scope assertion beside EXPECTED_MOON_CI_INVOCATIONS is what keeps that true (D8).

    THIS REVERSES SMA-541's explicit decision that "argument ORDER is not the property worth
    pinning". Under an exact literal, order, inter-word spacing, trailing comments, trailing
    whitespace and indentation are all pinned. That reversal costs less than it reads: check 8b in
    ci/actionlint/run.sh already matches each `"${T[@]}"`-carrying line against an exact allowlist,
    so a reordered or multi-space invocation is red in this repository today. What changes here is
    one gate's fixtures, not the repo's behaviour (SMA-554 E3).

    What this does NOT prove is that the block EXECUTES. A step-level `if: ${{ false }}`, or an
    `if false; then … fi` wrap, leaves all eight lines byte-identical and the count at 2. Check 8d
    (ci/actionlint/run.sh, block_execution_verdict) is the control that closes those, by executing
    the block against a stubbed `moon` on four GitHub event paths — see L2 in the spec and L12 in
    ci/actionlint/README.md. Do not delete 8d on the grounds that this function pins the same lines.

    Lines are split on "\\n" rather than with .splitlines(), which also splits on \\x0b, \\x0c,
    \\x1c-\\x1e, U+2028 and U+2029 — MOON_CI_LINE_RE's re.MULTILINE anchors split only on "\\n", so
    the two halves of this one check would otherwise disagree about what a line is. CRLF needs no
    handling: read_input uses Path.read_text(), whose universal-newline translation has already
    turned "\\r\\n" into "\\n" before any check sees the text (measured, SMA-554 E6).
    """
    rows = []
    lines = ci_yml_text.split("\n")

    # --- A: the anchored block pin ---
    start = _block_anchor(lines)
    if start is None:
        rows.append(
            "could not locate the `T=(...)` anchor line in .github/workflows/ci.yml, so the "
            "`moon ci` block's position could not be checked (expected exactly one such line)"
        )
    else:
        actual = lines[start:start + len(MOON_CI_BRANCH_BLOCK)]
        if actual != list(MOON_CI_BRANCH_BLOCK):
            diff = difflib.unified_diff(
                list(MOON_CI_BRANCH_BLOCK), actual,
                fromfile="MOON_CI_BRANCH_BLOCK (expected)",
                tofile=f".github/workflows/ci.yml line {start + 1}+ (actual)",
                lineterm="",
            )
            rows.append(
                "the `moon ci` branch block does not match MOON_CI_BRANCH_BLOCK verbatim:\n"
                + "\n".join("      " + d for d in diff)
            )

    # --- B: the extras counter ---
    found = len(MOON_CI_LINE_RE.findall(ci_yml_text))
    if found != EXPECTED_MOON_CI_INVOCATIONS:
        rows.append(
            f"found {found} command-position `moon ci` invocation(s), expected "
            f"{EXPECTED_MOON_CI_INVOCATIONS}. An ADDED invocation is the one thing the block pin "
            f"above cannot see; a deliberate new one means updating EXPECTED_MOON_CI_INVOCATIONS."
        )
    return rows
```

- [ ] **Step 6: Add the `difflib` import**

`ci_targets.py:19-25` holds the import block, alphabetically ordered. `difflib` sorts before `inspect`, so it goes first in that group:

```python
import difflib
import inspect
import json
import re
import subprocess
import sys
```

Ruff's isort rules enforce that ordering — putting it anywhere else in the group reds `repo:ruff-ci`.

- [ ] **Step 7: Run the self-test and verify the probe now passes**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 ci/affected-graph/ci_targets.py --self-test 2>&1 | tail -30
```
Expected: the two probe rows no longer appear. **Other `check_invocation` fixtures WILL now fail** — the pre-existing ones at `:1886-1962` mutate a hand-written 7-line string that does not contain the block, so under A they all red. That is expected and is exactly what Task 4 fixes. Note which ones fail; do not "fix" them here.

- [ ] **Step 8: Verify the real file still passes both assertions**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 -c "
import sys, pathlib; sys.path.insert(0, 'ci/affected-graph')
import ci_targets as m
rows = m.check_invocation(pathlib.Path('.github/workflows/ci.yml').read_text())
print('rows:', rows if rows else 'CLEAN')
"
```
Expected: `rows: CLEAN`. If it is not clean, the constant does not match the file — go back to Task 1 Step 7.

- [ ] **Step 9: Verify the error message is actually useful**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 -c "
import sys, pathlib; sys.path.insert(0, 'ci/affected-graph')
import ci_targets as m
t = pathlib.Path('.github/workflows/ci.yml').read_text()
t = t.replace('moon ci \"\${T[@]}\" --base origin/main', 'moon ci \"\${T[@]:0:5}\" --base origin/main')
for r in m.check_invocation(t): print(r)
"
```
Expected: a unified diff naming `MOON_CI_BRANCH_BLOCK (expected)` and the `ci.yml` line number, with `-` / `+` lines showing the subsetted expansion.

- [ ] **Step 10: Commit**

```bash
cd /Users/sven/dev/paigasus/paigasus-core
git add ci/affected-graph/ci_targets.py
git commit -m "ci(repo): anchor check_invocation to an exact-literal block (SMA-554)

Replaces the pattern-matched shape rule with a positionally-anchored
exact-literal comparison against MOON_CI_BRANCH_BLOCK, reported as a
unified diff. The anchor closes the decoy family: a verbatim copy pasted
elsewhere cannot bring its own T= line along, since parse_t rejects a
second one. Deletes _strip_comment.

The pre-existing fixtures fail after this commit and are re-based on the
constant in the next one.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ"
```

---

### Task 3: The fix message and the stale CRLF comment

**Files:**
- Modify: `ci/affected-graph/ci_targets.py:3074-3086` (the `bad_invocation` message)
- Modify: `ci/affected-graph/ci_targets.py:59-66` (`T_ARRAY_RE`'s CRLF comment)

**Interfaces:**
- Consumes: `MOON_CI_BRANCH_BLOCK` (Task 1).
- Produces: nothing new. The message must contain the literal string `MOON_CI_BRANCH_BLOCK` — Task 4's meta-fixture asserts it.

- [ ] **Step 1: Replace the `bad_invocation` message**

At `:3074-3086`, the current tuple entry references the deleted `MOON_CI_INVOCATION` and describes row shapes that no longer exist. Replace the whole `(bad_invocation, "…")` entry with:

```python
        (bad_invocation,
         "`.github/workflows/ci.yml`'s `moon ci` branch block no longer matches the exact literal\n"
         "    pinned as `MOON_CI_BRANCH_BLOCK` in ci/affected-graph/ci_targets.py, or the file\n"
         "    carries a `moon ci` invocation outside it. Every other check asserts what is IN `T`;\n"
         "    this one asserts `T` is what runs.\n"
         "    If the edit was DELIBERATE, it has FOUR co-update sites and all four must move\n"
         "    together (SMA-554):\n"
         "      1. .github/workflows/ci.yml               — the block itself\n"
         "      2. ci/actionlint/run.sh                   — T_INVOCATION_ALLOWLIST (check 8b)\n"
         "      3. ci/actionlint/run.sh                   — block_execution_verdict (check 8d)\n"
         "      4. ci/affected-graph/ci_targets.py        — MOON_CI_BRANCH_BLOCK\n"
         "    Copy the lines VERBATIM from ci.yml, indentation included; do not hand-format them.\n"
         "    If instead this is an added invocation, `EXPECTED_MOON_CI_INVOCATIONS` is the\n"
         "    constant to review — deliberately, not reflexively."),
```

- [ ] **Step 2: Correct `T_ARRAY_RE`'s CRLF comment**

The comment at `:59-66` claims a CRLF checkout reds with a misleading message. Spec E6 measured that unreachable — `Path.read_text()` translates newlines first. Replace the CRLF sentences (keeping the first two sentences about `[ \t]*$` vs `\s*$`) so the block reads:

```python
# The canonical single-line array. `[ \t]*$` rather than `\s*$` is DEFENSIVE, not load-bearing:
# `(.*?)` cannot cross a newline without re.DOTALL, so `\s*$` would not in fact accept a multi-line
# array either.
#
# An earlier version of this comment claimed the stricter anchor changes CRLF behaviour, reddening
# with a misleading "must stay on one line" message on a CRLF checkout. That state is UNREACHABLE
# and the claim is withdrawn (measured, SMA-554 E6): read_input uses Path.read_text(), i.e. text
# mode with newline=None, so universal-newline translation has already turned "\r\n" into "\n"
# before this regex — or any other check in this file — sees the text.
T_ARRAY_RE = re.compile(r"^[ \t]*T=\((.*?)\)[ \t]*$", re.MULTILINE)
```

- [ ] **Step 3: Verify the module imports and the message renders**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
import ci_targets
print('import ok')
" && grep -c "MOON_CI_BRANCH_BLOCK" ci/affected-graph/ci_targets.py
```
Expected: `import ok`, then a count of at least 4 (the constant, the check, the message, the docstring).

- [ ] **Step 4: Commit**

```bash
cd /Users/sven/dev/paigasus/paigasus-core
git add ci/affected-graph/ci_targets.py
git commit -m "ci(repo): rewrite the invocation fix message and drop a false CRLF claim (SMA-554)

The old message named the deleted MOON_CI_INVOCATION and described row
shapes that no longer exist. The new one enumerates all four co-update
sites, so a human editing the block is told every place that must move
together rather than discovering them one red at a time.

T_ARRAY_RE's CRLF paragraph described an unreachable state: read_text
translates newlines before any check sees the file.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ"
```

---

### Task 4: The fixture battery

**Files:**
- Modify: `ci/affected-graph/ci_targets.py:1883-1962` (replace the whole invocation fixture region, and the Task 2 probe)

**Interfaces:**
- Consumes: `check_invocation` (Task 2), `MOON_CI_BRANCH_BLOCK` (Task 1).
- Produces: nothing consumed by later tasks.

**Why the existing fixtures cannot be kept:** they mutate a hand-written 7-line string (`invoked`, `:1886-1894`) with no `elif` branch and no comment line. Under assertion A every mutation of it reds because the canonical block was never there — the mutation asserts nothing. That is the SMA-526 vacuous-fixture class. Every fixture below is **derived** from the constant, and every red row is paired with a **revert-to-green twin** proving the mutation, not the scaffolding, is what reds it.

- [ ] **Step 1: Delete the old region and the Task 2 probe**

Delete everything from the `# TASK 2 PROBE` comment through the `failures.append("check_invocation: fired on a comment or a `name:` field")` block — i.e. the whole current invocation fixture region ending just before the `# A DELETED input file is an authorial mistake (rc 1)` comment.

- [ ] **Step 2: Write the replacement battery**

Insert in its place:

```python
    # The invocation pin (SMA-554). Every fixture text is DERIVED from MOON_CI_BRANCH_BLOCK rather
    # than hand-written: a hand-written near-copy would red under assertion A because the canonical
    # block was never present, so the mutation would assert nothing — the SMA-526 vacuous-fixture
    # class. Each red row below is paired with a `clean()` twin on the UNMUTATED text, which is
    # what proves the mutation is load-bearing.
    ANCHOR = "          T=(:build :test)"
    canonical_block = "\n".join(MOON_CI_BRANCH_BLOCK)

    def step(block, prologue="", epilogue=""):
        """A synthetic ci.yml step: prologue, the T= anchor, `block`, then epilogue."""
        return (
            "      - name: moon ci (affected graph)\n"
            "        run: |\n"
            "          set -euo pipefail\n"
            + prologue + ANCHOR + "\n" + block + "\n" + epilogue
        )

    def red(label, text):
        if not check_invocation(text):
            failures.append(f"check_invocation[{label}]: reported clean, expected red")

    def clean(label, text):
        got = check_invocation(text)
        if got:
            failures.append(f"check_invocation[{label}]: reported red, expected clean: {got}")

    def mutate(label, old, new, prologue="", epilogue=""):
        """Assert `old`->`new` reds, and that the same scaffolding UNMUTATED is clean."""
        if old not in canonical_block:
            failures.append(f"check_invocation[{label}]: fixture anchor {old!r} not in the block")
            return
        red(label, step(canonical_block.replace(old, new), prologue, epilogue))
        clean(f"{label}/twin", step(canonical_block, prologue, epilogue))

    PR_LINE = '            moon ci "${T[@]}" --base origin/main --include-relations'

    # 18 — the canonical block at its anchor. A TAUTOLOGY by construction: it is built from the
    # constant the check compares against, so it cannot fail while the check is coherent. Its
    # non-vacuous twin is the production run against the real ci.yml (AC #4), not this row. Kept
    # because it is the baseline every `mutate()` twin above depends on.
    clean("canonical", step(canonical_block))

    # 1-5 — the four bypasses SMA-541's regex was defeated by, each verified real at the time.
    mutate("subsetted", '"${T[@]}" --base origin/main', '"${T[@]:0:5}" --base origin/main')
    mutate("leading-flag", PR_LINE, '            moon ci --base origin/main "${T[@]:0:5}"')
    mutate("echo-prefixed", PR_LINE, "            echo " + PR_LINE.strip())
    mutate("multi-space", PR_LINE, '            moon    ci "${T[@]:0:5}" --base origin/main')
    mutate("trailing-comment", PR_LINE, PR_LINE + '  # restore "${T[@]}" later')

    # 6-7 — expansions that leave `T` itself perfectly correct.
    mutate("unquoted-T", 'moon ci "${T[@]}" --base origin/main', "moon ci $T --base origin/main")
    mutate("bypasses-T", 'moon ci "${T[@]}" --base origin/main', "moon ci :build --base origin/main")

    # 8 — a deleted invocation. Reds under BOTH assertions, which is the point: the issue's sketch
    # believed only the counter could see this, and the block pin sees it too (SMA-554 E2).
    deleted = step(canonical_block.replace(PR_LINE + "\n", ""))
    red("deleted-invocation", deleted)
    if len(MOON_CI_LINE_RE.findall(deleted)) == EXPECTED_MOON_CI_INVOCATIONS:
        failures.append("check_invocation[deleted-invocation]: the counter did not also see it")

    # 9-10 — what ONLY the counter can see. The block is byte-identical in both, so assertion A is
    # green and B is carrying the whole row.
    for label, extra in (
        ("third-invocation", '          moon ci "${T[@]}" --base origin/main --include-relations'),
        ("dead-branch-subset", '          moon ci "${T[@]:0:5}" --base origin/main'),
    ):
        text = step(canonical_block, epilogue=extra + "\n")
        red(label, text)
        if check_invocation(step(canonical_block, epilogue=extra + "\n")) != check_invocation(text):
            failures.append(f"check_invocation[{label}]: not deterministic")
    clean("no-extra/twin", step(canonical_block))

    # 11-12, 14 — properties the old pattern-based check could not hold at all.
    mutate("re-indented", PR_LINE, "  " + PR_LINE)
    mutate("trailing-whitespace", PR_LINE, PR_LINE + " ")
    red("line-inserted-mid-block",
        step(canonical_block.replace(PR_LINE, PR_LINE + "\n            echo interposed")))

    # 13 — E3's three reversals. These asserted CLEAN before SMA-554 and now assert RED. That is
    # deliberate, not a regression: SMA-541 reasoned "argument ORDER is not the property worth
    # pinning", which is true of a shape rule and false of an exact literal. It costs less than it
    # reads — check 8b in ci/actionlint/run.sh matches each `"${T[@]}"`-carrying line against an
    # exact allowlist, so all three are ALREADY red in this repository (SMA-554 E3). Kept as
    # inverted fixtures rather than deleted so the reversal cannot be re-litigated as a bug.
    mutate("reordered-canonical", PR_LINE, '            moon ci --base origin/main "${T[@]}" --include-relations')
    mutate("multi-space-intact", PR_LINE, '            moon    ci "${T[@]}" --base origin/main --include-relations')
    mutate("comment-on-correct-line", PR_LINE, PR_LINE + "  # PR path")

    # 15-16 — the DECOY family, which is why assertion A is anchored (D7). Both keep a verbatim
    # copy of the eight lines somewhere in the file; neither may satisfy the pin.
    red("decoy-copy-elsewhere",
        step(canonical_block.replace('"${T[@]}" --base origin/main', '"${T[@]:0:5}" --base origin/main'),
             epilogue=canonical_block + "\n"))
    red("detached-from-anchor", step(canonical_block, prologue="", epilogue="").replace(
        ANCHOR + "\n", ANCHOR + "\n          echo interposed\n"))

    # 17 — the message must name the constant to update, or AC #2 ships unverified.
    msg_rows = check_invocation(step(canonical_block.replace('"${T[@]}"', '"${T[@]:0:5}"')))
    if not any("MOON_CI_BRANCH_BLOCK" in row for row in msg_rows):
        failures.append(
            f"check_invocation: the failure message does not name MOON_CI_BRANCH_BLOCK: {msg_rows}"
        )

    # 19 — prose comments and `name:` fields mentioning `moon ci` must not be counted. The real
    # ci.yml carries eight of the former and two of the latter.
    clean("prose-and-name-fields", step(
        canonical_block,
        prologue="          # `moon ci` is affected-only, so a PR touching no Rust never rebuilds\n",
        epilogue="      - name: moon ci (affected graph)\n",
    ))

    # 20 — documents L1 in the both-directions style this file already uses. `$MOON ci` is NOT at
    # command position for MOON_CI_LINE_RE, so an added invocation spelled that way is invisible
    # here. Check 8d catches it behaviourally by executing the block; this gate does not, and the
    # fixture says so out loud rather than leaving it to be rediscovered.
    clean("L1-uncounted-indirection",
          step(canonical_block, epilogue='          $MOON ci "${T[@]:0:5}"\n'))

    # The D8 invariant, re-asserted here as well as at module scope: an `assert` statement is
    # stripped under `python -O`, and this battery is the gate's proof-that-it-bites.
    if sum(1 for line in MOON_CI_BRANCH_BLOCK if MOON_CI_LINE_RE.match(line)) != EXPECTED_MOON_CI_INVOCATIONS:
        failures.append("MOON_CI_BRANCH_BLOCK and EXPECTED_MOON_CI_INVOCATIONS disagree")
```

- [ ] **Step 3: Run the self-test**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
python3 ci/affected-graph/ci_targets.py --self-test
```
Expected: `PASS` with no `check_invocation[...]` rows. If a `mutate()` twin fails, the scaffolding is wrong, not the check.

- [ ] **Step 4: Prove the battery is not vacuous**

A green battery over a broken check is the failure this task exists to prevent. Break the check on purpose and confirm the fixtures notice:

```bash
cd /Users/sven/dev/paigasus/paigasus-core
cp ci/affected-graph/ci_targets.py /tmp/ci_targets.bak
python3 - <<'PY'
import pathlib
p = pathlib.Path("ci/affected-graph/ci_targets.py")
t = p.read_text()
# Neuter assertion A: compare against itself instead of the file.
t = t.replace("if actual != list(MOON_CI_BRANCH_BLOCK):", "if False:", 1)
p.write_text(t)
PY
python3 ci/affected-graph/ci_targets.py --self-test 2>&1 | tail -25
cp /tmp/ci_targets.bak ci/affected-graph/ci_targets.py
python3 ci/affected-graph/ci_targets.py --self-test | tail -3
```
Expected: the mutated run FAILS, listing many `check_invocation[...]: reported clean, expected red` rows; the restored run passes. If the mutated run passes, the battery is vacuous — stop and fix it.

- [ ] **Step 5: Run the whole gate against the real tree**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py
```
Expected: `PASS  ci-targets         -> 30 targets: every CI-eligible repo task is in ci.yml's T, …`

- [ ] **Step 6: Lint**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py ruff check ci/affected-graph/ci_targets.py
```
Expected: `All checks passed!`. Fix any finding — do **not** run `ruff format`.

- [ ] **Step 7: Commit**

```bash
cd /Users/sven/dev/paigasus/paigasus-core
git add ci/affected-graph/ci_targets.py
git commit -m "ci(repo): re-base the invocation fixtures on the pinned constant (SMA-554)

The old fixtures mutated a hand-written 7-line string that does not
contain the block, so under the exact-literal pin every mutation would
have redded vacuously. Each fixture is now derived from
MOON_CI_BRANCH_BLOCK and paired with a revert-to-green twin proving the
mutation is what reds it.

Adds rows the pattern-based check could not hold: re-indentation,
trailing whitespace, mid-block insertion, and the two decoy shapes the
anchor exists to close. Three rows are inverted from clean to red and
say why in place.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ"
```

---

### Task 5: Documentation

**Files:**
- Modify: `ci/affected-graph/README.md:295-315`
- Modify: `docs/superpowers/specs/2026-08-19-sma-541-ci-target-coverage-gate-design.md:257-286, :356-358, :432`

**Interfaces:** none — documentation only.

- [ ] **Step 1: Read the current README region**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
sed -n 293,316p ci/affected-graph/README.md
```

- [ ] **Step 2: Rewrite C5's README entry**

Replace the `**C5** every …` clause at `:301` and the "C5's line matcher is …" sentence at `:311` so C5 reads:

> **C5** the `moon ci` branch block in `ci.yml` matches `MOON_CI_BRANCH_BLOCK` verbatim — eight
> lines, indentation included — beginning immediately after the sole `T=(…)` line, and the file
> carries exactly two command-position `moon ci` lines. Exact literals replaced a regex-based
> shape rule in SMA-554, after that rule was bypassed four times during SMA-541's own review;
> an exact comparison has no tail to enumerate. The anchor closes the decoy case: a verbatim copy
> pasted elsewhere cannot bring its own `T=` line, because `parse_t` rejects a second one.
>
> C5 is a **second opinion, not the primary guard**. `ci/actionlint/run.sh`'s check 8b already
> pins the same three invocation lines as exact literals, and check 8d **executes** the block
> against a stubbed `moon` on four GitHub event paths — which is the only control that sees a
> step-level `if: false` or an `if false; then … fi` wrap, since both leave every line
> byte-identical. C5's value is that it is scheduled independently of `repo:actionlint`. Editing
> those eight lines therefore has **four** co-update sites; C5's failure message lists them all.

- [ ] **Step 3: Annotate SMA-541's C5 entry as superseded**

Insert immediately before the `- **C5 — invocation shape.**` bullet at `:257`:

```markdown
> **Superseded by [SMA-554](https://linear.app/smaschek/issue/SMA-554) (2026-09-03).** The line
> matcher described below was replaced by an exact-literal, anchored block pin
> (`MOON_CI_BRANCH_BLOCK`). The reasoning is kept verbatim because it records *why* pattern
> matching was tried and how each of its four bypasses was found — that history is the evidence
> for the replacement. One decision below is explicitly reversed: "argument order is not the
> property worth pinning" no longer holds, and SMA-554 E3 records that `repo:actionlint`'s check
> 8b already behaved the new way when this was written.
```

- [ ] **Step 4: Annotate the fixture table rows**

At `:356-358`, append a footnote row under the table:

```markdown
> **SMA-554:** the last two rows now read *C5 red* rather than green — reordered-but-canonical and
> multi-space-intact invocations are red under an exact literal. See SMA-554 E3 for why that costs
> less than it reads.
```

- [ ] **Step 5: Annotate L10 as STILL OPEN**

At `:432`, append to the L10 bullet:

```markdown
  **Still open after SMA-554** — and load-bearing, so do not read the exact-literal pin as closing
  it. A step-level `if: ${{ false }}` leaves all eight pinned lines byte-identical and the
  invocation count at 2, so neither of C5's assertions can see it. `repo:actionlint`'s check 8d is
  the control that does, by executing the block; see L12 in `ci/actionlint/README.md`.
```

- [ ] **Step 6: Verify no stale references survive**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
grep -rnw "MOON_CI_INVOCATION\|T_ARRAY_EXPANSION\|_strip_comment" ci/ || echo "NONE — clean"
```
Expected: `NONE — clean`.

Two things make this command actually able to print that, and both are load-bearing.
`-w` is required because `MOON_CI_INVOCATION` is a proper **substring** of the still-live
`EXPECTED_MOON_CI_INVOCATIONS`, so an unanchored pattern matches ten surviving lines and can never
report clean. And the search is scoped to `ci/` rather than the whole tree because
`docs/superpowers/` legitimately keeps all three names — this plan quotes them in its own task
text, and the SMA-541 and SMA-554 specs record them as history. What the check is really asserting
is that no **live code** reference survives.

- [ ] **Step 7: Commit**

```bash
cd /Users/sven/dev/paigasus/paigasus-core
git add ci/affected-graph/README.md docs/superpowers/specs/2026-08-19-sma-541-ci-target-coverage-gate-design.md
git commit -m "docs(repo): document the invocation literal pin and its limits (SMA-554)

Rewrites C5's README entry and says plainly that it is a second opinion
rather than the primary guard, since checks 8b and 8d already cover
these lines and 8d is the only one that proves the block executes.

Annotates SMA-541's C5 entry as superseded while keeping its reasoning
verbatim, and marks its L10 as STILL OPEN — an exact-literal pin cannot
see a step-level if: false any more than a regex could.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_014mASjm4Xwz4fyKunCz2BSZ"
```

---

### Task 6: Full-graph verification

**Files:** none modified. This task is acceptance only.

**Interfaces:** none.

- [ ] **Step 1: Run the affected-smoke gate with its negative control**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force 2>&1 | tail -40
```
Expected: the negative control reports its deliberately-wrong expectations as red, then the real suite passes. **If this aborts in under 3 seconds**, capture the full output before re-running — CLAUDE.md records a rare `proto-shim … Permission denied` abort whose evidence a re-run destroys. Grep the captured output for `proto-shim`; if present the failure is infrastructure, not this change, and `moon run repo:affected-smoke --force` again will pass.

- [ ] **Step 2: Run the actionlint gate — 8b and 8d must be unaffected**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force 2>&1 | tail -20
```
Expected: PASS. This change touches no workflow file, so any failure here is unrelated — investigate before proceeding.

- [ ] **Step 3: Run the ruff gate over `ci/`**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:ruff-ci --force 2>&1 | tail -20
```
Expected: PASS.

- [ ] **Step 4: Run the full CI target graph**

The per-project tasks do not run the repo-level gates; run the graph the way CI does. Copy the command from CLAUDE.md's marker-delimited block verbatim:

```bash
cd /Users/sven/dev/paigasus/paigasus-core
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  --base origin/main --include-relations 2>&1 | tail -40
```
Expected: all tasks pass. Note that the three `repo:release-parity*` tasks abort **inconclusive at rc=2** inside an agent session because `proto` emits NDJSON — that is documented behaviour, not a failure of this change. Read the output rather than the exit code for those three.

- [ ] **Step 5: Confirm the working tree is clean and the diff matches the plan**

Run:
```bash
cd /Users/sven/dev/paigasus/paigasus-core
git status --short
git diff origin/main --stat
```
Expected: a clean tree, and exactly four files changed — `ci/affected-graph/ci_targets.py`, `ci/affected-graph/README.md`, the SMA-541 spec, and the two SMA-554 docs. **`.github/workflows/ci.yml` must NOT appear.** If it does, the block was edited when it should not have been — revert that file.

---

## Self-review

**Spec coverage.** §3's A → Task 2 (`check_invocation` + `_block_anchor`); §3's B → Task 1 Step 4 + Task 2 Step 5; the three deletions → Task 1 Step 2 and Task 2 Step 3. D1 → Task 1 Step 3's verbatim-copy comment; D2 → Task 1 Step 4; D3 → Task 2 Step 5's `split("\n")`; D4 → Task 2 Step 5's `difflib` + Task 3 Step 1's message + Task 4 fixture 17; D5 → no task, deliberately (`assert_include_relations` is left alone); D6 → Task 4's three inverted rows; D7 → Task 2 Step 4; D8 → Task 1 Step 5 and Task 4's closing row; D9 → Task 1 Step 3 (tuple); D10 → Task 4's decoy rows. §7's fixtures 1-20 → Task 4, numbered to match. §7's acceptance → Task 6. §8's six documentation targets → Task 3 Steps 1-2 (the two `ci_targets.py` ones) and Task 5 (the rest); `CLAUDE.md` needs no change, per the spec. §9's L1 → Task 4 fixture 20; L2 → Task 2's docstring and Task 5 Step 5; L3 and L4 are stated limitations with no task, correctly.

**Placeholder scan.** No TBDs. Every code step carries the literal text to insert. Task 6 is verification-only and says so.

**Type consistency.** `check_invocation(ci_yml_text) -> list[str]` keeps its signature, so `main()`'s call at `:2958` and the `bad_invocation` truthiness test are unchanged. `_block_anchor(lines) -> int | None` is consumed only inside `check_invocation`. `MOON_CI_BRANCH_BLOCK` is a tuple everywhere; `list(...)` conversions are explicit at both comparison sites.

**One ordering note for the executor.** Task 2 Step 7 leaves the self-test **failing** on the pre-existing fixtures, by design — they are re-based in Task 4. Do not attempt to repair them in Task 2, and do not treat that red as a blocker between the two commits.
