# SMA-436 — Fix `py:typecheck` Vacuous Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `py:typecheck` gate actually type-check the Python source tree and stay non-vacuous, by fixing the basedpyright `include` globs and adding a durable coverage-floor guard.

**Architecture:** Three changes. (1) Correct `tool.basedpyright.include` in `py/pyproject.toml` so basedpyright collects files (a single-`*` glob terminating at a directory matches the dir but collects none of its `.py` files; it needs a recursive `/**`). (2) Add a small Python guard that reads `basedpyright --outputjson` and fails when fewer files were analyzed than the source tree contains — catching total *and* partial darkening. (3) Rewire the Moon `typecheck` task to run basedpyright (native output) then pipe a JSON-only pass through the guard. A one-shot canary proves both the fix and the guard before merge.

**Tech Stack:** Moon 2.3.2 tasks (`.moon/tasks/python.yml`), basedpyright 1.39.8 (`--outputjson`), uv, Python 3.12 stdlib.

**Spec:** `docs/superpowers/specs/2026-06-20-sma-436-py-typecheck-vacuous-pass-design.md`

---

## Conventions for every command in this plan

- Run all commands from the **`py/`** directory unless stated otherwise.
- Ensure proto-managed tools (`uv`, `moon`) are on PATH first:

  ```bash
  export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
  ```

  (Shims first = repo-pinned versions. `uv`/`moon` are proto-managed and are **not** on the default PATH.)
- Branch is already `feature/sma-436-fix-pytypecheck-vacuous-pass-basedpyright-checks-zero-files`.
- Every source file opens with `# SPDX-License-Identifier: Apache-2.0`.

## File Structure

| File | Responsibility | Action |
| --- | --- | --- |
| `py/scripts/assert_typecheck_coverage.py` | Read `basedpyright --outputjson` from stdin; fail if `filesAnalyzed` is below the on-disk source-file count (derived floor); fail-closed with a distinct code on unreadable JSON. Pure stdlib, tooling only — not type-checked, but ruff-linted. | Create |
| `py/pyproject.toml` | `tool.basedpyright.include` glob fix (`/**` suffix) + an explanatory comment. | Modify |
| `.moon/tasks/python.yml` | `typecheck` task: `command:` → `script:` (basedpyright + guarded JSON pass); add `scripts/**` input. | Modify |

Ordering keeps every commit green: the guard script lands unused first (Task 1), then the include fix makes the gate real (Task 2), then wiring the guard adds the durable check (Task 3) — the guard is only ever wired in *after* the include already yields ≥6 files, so it never commits a red gate.

---

### Task 1: Create the coverage-floor guard script

**Files:**
- Create: `py/scripts/assert_typecheck_coverage.py`

The guard's behavioral contract (verified by piping known JSON in Step 2/4) is:

| stdin | exit code | meaning |
| --- | --- | --- |
| `{"summary":{"filesAnalyzed":N}}` with `N >= on-disk count` | `0` | gate saw enough files |
| `{"summary":{"filesAnalyzed":N}}` with `N < on-disk count` | `1` | (partial) vacuous gate |
| empty / non-JSON / missing key | `2` | unreadable `--outputjson` (fail-closed) |

- [ ] **Step 1: Write the guard script**

Create `py/scripts/assert_typecheck_coverage.py` with exactly this content:

```python
# SPDX-License-Identifier: Apache-2.0
"""Coverage-floor guard for the py:typecheck gate (SMA-436).

basedpyright passes vacuously when its `include` globs match no files (it exits 0 on
"No source files found"). This guard reads `basedpyright --outputjson` from stdin and fails
the gate unless basedpyright analyzed at least as many files as the source tree actually
contains -- catching both total and partial darkening of the type gate.

The expected count mirrors `tool.basedpyright.{include,exclude}` in py/pyproject.toml; the two
must move together. Intended use (pipe), with py/ as the project root:

    uv run basedpyright --outputjson | uv run python scripts/assert_typecheck_coverage.py
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

# Mirrors tool.basedpyright.include (src + tests per package) and the exclude basenames.
INCLUDE_GLOBS = ("packages/*/src", "packages/*/tests")
EXCLUDE_PARTS = frozenset({"generated", "__pycache__", "node_modules", ".venv", "dist", "build"})

EXIT_OK = 0
EXIT_UNDER_FLOOR = 1
EXIT_UNREADABLE = 2


def expected_count(py_root: Path) -> int:
    """Count the .py files the type gate is intended to cover, read from the filesystem."""
    files: set[Path] = set()
    for pattern in INCLUDE_GLOBS:
        for base in py_root.glob(pattern):
            for path in base.rglob("*.py"):
                if EXCLUDE_PARTS.isdisjoint(path.parts):
                    files.add(path)
    return len(files)


def main() -> int:
    raw = sys.stdin.read()
    try:
        analyzed = int(json.loads(raw)["summary"]["filesAnalyzed"])
    except (json.JSONDecodeError, KeyError, TypeError, ValueError):
        print(
            "py:typecheck coverage guard: could not read basedpyright --outputjson "
            "(empty output or unexpected schema?). Is basedpyright crashing, or did its "
            "--outputjson schema change under the basedpyright<2 pin?",
            file=sys.stderr,
        )
        return EXIT_UNREADABLE

    # scripts/ lives directly under py/, so the parent of this file's dir is the py root.
    expected = expected_count(Path(__file__).resolve().parents[1])
    if analyzed < expected:
        print(
            f"py:typecheck coverage guard: basedpyright analyzed {analyzed} file(s) but the "
            f"source tree contains {expected}. The type gate is (partially) vacuous -- check "
            f"tool.basedpyright.include in py/pyproject.toml (each glob needs a recursive /** "
            f"suffix; see SMA-436).",
            file=sys.stderr,
        )
        return EXIT_UNDER_FLOOR

    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 2: Verify the guard behaves correctly (these checks stand in for unit tests)**

Run from `py/`:

```bash
echo '{"summary":{"filesAnalyzed":6}}'    | uv run python scripts/assert_typecheck_coverage.py; echo "exit=$?"
echo '{"summary":{"filesAnalyzed":5}}'    | uv run python scripts/assert_typecheck_coverage.py; echo "exit=$?"
echo '{"summary":{"filesAnalyzed":0}}'    | uv run python scripts/assert_typecheck_coverage.py; echo "exit=$?"
echo 'not json'                            | uv run python scripts/assert_typecheck_coverage.py; echo "exit=$?"
printf ''                                  | uv run python scripts/assert_typecheck_coverage.py; echo "exit=$?"
```

Expected:
- `filesAnalyzed:6` → `exit=0` (at/above the floor; also proves `expected_count == 6`, not higher)
- `filesAnalyzed:5` → `exit=1` (below floor; proves `expected_count == 6`, not lower)
- `filesAnalyzed:0` → `exit=1` (total vacuity)
- `not json` → `exit=2` with the "could not read … schema changed?" message on stderr
- empty stdin → `exit=2`

If `6` does not give `exit=0` or `5` does not give `exit=1`, the on-disk count differs from 6 — list it with:
`uv run python -c "import sys; sys.path.insert(0,'scripts'); import assert_typecheck_coverage as g; from pathlib import Path; print(g.expected_count(Path('.').resolve()))"`
and reconcile against `uv run basedpyright --outputjson packages` (`summary.filesAnalyzed`) before continuing.

- [ ] **Step 3: Lint and format the new script**

Run from `py/`:

```bash
uv run ruff format scripts/assert_typecheck_coverage.py
uv run ruff check scripts/assert_typecheck_coverage.py
```

Expected: ruff format reports the file unchanged or reformats it in place; `ruff check` reports `All checks passed!`. Fix any reported lint before committing.

- [ ] **Step 4: Re-run the behavioral checks after formatting**

Re-run the Step 2 command block. Expected: identical exit codes (`0,1,1,2,2`). (Formatting must not change behavior.)

- [ ] **Step 5: Commit**

```bash
cd .. && git add py/scripts/assert_typecheck_coverage.py
git commit -m "feat(py): add typecheck coverage-floor guard (SMA-436)

Stdlib guard that reads basedpyright --outputjson and fails when fewer files
were analyzed than the source tree contains. Not yet wired into the gate."
```

---

### Task 2: Fix the basedpyright `include` glob

**Files:**
- Modify: `py/pyproject.toml` (the `include = [...]` line under `[tool.basedpyright]`)

- [ ] **Step 1: Confirm the gate is currently vacuous (red baseline)**

Run from `py/`:

```bash
uv run basedpyright --outputjson | uv run python -c "import sys,json; print('filesAnalyzed =', json.load(sys.stdin)['summary']['filesAnalyzed'])"
```

Expected: `filesAnalyzed = 0` (the bug). This is the failing state the fix must flip.

- [ ] **Step 2: Apply the include fix**

In `py/pyproject.toml`, replace this line:

```toml
include = ["packages/*/src", "packages/*/tests"]
```

with this comment + line:

```toml
# Each glob needs a recursive /** suffix: a single-* pattern terminating at a directory
# (packages/*/src) matches the dir but collects NONE of its .py files, so basedpyright runs on
# zero files and the gate passes vacuously (SMA-436). Do not drop the /**.
include = ["packages/*/src/**", "packages/*/tests/**"]
```

- [ ] **Step 3: Verify basedpyright now analyzes the source tree and is clean**

Run from `py/`:

```bash
uv run basedpyright --outputjson | uv run python -c "import sys,json; s=json.load(sys.stdin)['summary']; print('filesAnalyzed =', s['filesAnalyzed'], 'errorCount =', s['errorCount'])"
uv run basedpyright; echo "exit=$?"
```

Expected: `filesAnalyzed = 6 errorCount = 0`, and the bare run prints `0 errors, 0 warnings, 0 notes` with `exit=0`. (The gate is now real and green — no hidden backlog, per the spec.)

- [ ] **Step 4: Commit**

```bash
cd .. && git add py/pyproject.toml
git commit -m "fix(py): basedpyright include globs now match source files (SMA-436)

Single-* globs terminating at a directory collect no .py files; add the
recursive /** suffix so py:typecheck checks packages/*/src and tests (6 files)."
```

---

### Task 3: Wire the coverage guard into the Moon `typecheck` task

**Files:**
- Modify: `.moon/tasks/python.yml` (the `typecheck:` task)

- [ ] **Step 1: Replace the `typecheck` task**

In `.moon/tasks/python.yml`, replace this block:

```yaml
  typecheck:
    command: 'uv run basedpyright'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock']
```

with:

```yaml
  typecheck:
    # `script` (not `command`) because we chain two steps. First run: basedpyright's native
    # output, which fails on real type errors. Second run: --outputjson piped to the coverage
    # guard, which fails if basedpyright analyzed fewer files than the source tree contains --
    # so an include/layout regression can't silently re-darken the gate, totally or partially
    # (SMA-436). The guard is the last command in the pipe, so its exit status is the pipeline's.
    script: 'uv run basedpyright && uv run basedpyright --outputjson | uv run python scripts/assert_typecheck_coverage.py'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**']
```

- [ ] **Step 2: Run the gate through Moon (green path)**

Run from the repo root:

```bash
moon run py:typecheck --force
```

Expected: the task passes — basedpyright prints `0 errors, 0 warnings, 0 notes`, the guard emits nothing, and Moon reports the task succeeded. (`--force` bypasses Moon's cache so the new script actually runs.)

- [ ] **Step 3: Commit**

```bash
git add .moon/tasks/python.yml
git commit -m "ci(py): guard py:typecheck against vacuous (zero/under-floor) runs (SMA-436)

Run basedpyright, then assert filesAnalyzed >= on-disk source count via the
coverage guard. Adds scripts/** as a task input."
```

---

### Task 4: One-shot canary — prove the gate has teeth (no commit)

This is the spec's §3 verification. Nothing here is committed; every change is reverted.

- [ ] **Step 1: Prove the include fix catches real type errors**

From `py/`, inject a deliberate error into a tracked source file:

```bash
printf '\nbad: int = "not an int"\n' >> packages/paigasus-kernel/src/paigasus_kernel/__init__.py
```

Run from repo root: `moon run py:typecheck --force`

Expected: **RED** — basedpyright reports a `reportAssignmentType` error on the bad line and the task fails.

- [ ] **Step 2: Revert the injected error**

```bash
cd .. && git checkout -- py/packages/paigasus-kernel/src/paigasus_kernel/__init__.py
moon run py:typecheck --force
```

Expected: back to **GREEN** (`0 errors`, guard silent).

- [ ] **Step 3: Prove the coverage guard catches darkening**

Temporarily re-break the include by editing `py/pyproject.toml`: change the `include` line back to
the broken single-`*` form (do **not** touch the explanatory comment):

```toml
include = ["packages/*/src", "packages/*/tests"]
```

Run from repo root: `moon run py:typecheck --force`

Expected: **RED** — basedpyright itself reports `0 errors` (vacuous), but the guard fails the task with `basedpyright analyzed 0 file(s) but the source tree contains 6 …`.

- [ ] **Step 4: Restore the fix and confirm green**

Restore the committed (fixed) version exactly, from repo root:

```bash
git checkout -- py/pyproject.toml
git diff --quiet -- py/pyproject.toml && echo "pyproject restored cleanly"
moon run py:typecheck --force
```

Expected: `pyproject restored cleanly` (no diff vs the Task 2 commit) and the gate is **GREEN** again. Confirm `git status` shows a clean working tree (no stray edits).

---

### Task 5: Full affected gate + open the PR

- [ ] **Step 1: Run the affected build/test graph locally**

From repo root:

```bash
moon ci :typecheck
moon run py:lint py:fmt
```

Expected: all pass. (`py:fmt` is `ruff format --check` over the whole py tree; `py:lint` is `ruff check`. The new `scripts/` file is covered by both.)

- [ ] **Step 2: Push the branch**

```bash
git push -u origin feature/sma-436-fix-pytypecheck-vacuous-pass-basedpyright-checks-zero-files
```

- [ ] **Step 3: Open the PR**

```bash
gh pr create --fill --base main
```

In the PR body, record the Task 4 canary results (both RED proofs + restored GREEN) as evidence the gate now has teeth. Do **not** attach a Linear link — the integration auto-links by branch name. Confirm the prebuild/CI checks go green.

---

## Self-Review notes (already applied)

- **Spec coverage:** include fix (Task 2 ↔ design §1), durable coverage-floor guard with derived floor + fail-closed parsing (Tasks 1, 3 ↔ design §2), one-shot canary both directions (Task 4 ↔ design §3). Residual-risk/potency item is documentation-only (no task needed). All acceptance criteria map to a step.
- **Out of scope honored:** no change to `lint`/`fmt`/`test` tasks, `typeCheckingMode`, or `report*`; `py/conftest.py` left outside the gate; `generated/**` stays excluded (the guard mirrors that exclude basename).
- **Type/name consistency:** `expected_count(py_root)`, `INCLUDE_GLOBS`, `EXCLUDE_PARTS`, and the `EXIT_OK/UNDER_FLOOR/UNREADABLE` codes are used consistently across the script and the Task 1 behavioral table.
- **No placeholders:** every code step shows complete content; every run step states the expected output/exit code.
