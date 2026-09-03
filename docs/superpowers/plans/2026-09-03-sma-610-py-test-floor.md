# SMA-610 — py:test per-package collection floor — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `py:test` red when ONE `py/packages/*/tests` directory is moved, renamed or deleted, instead of silently collecting the survivors and exiting 0.

**Architecture:** A pure-stdlib guard (`py/scripts/assert_test_floor.py`) reads `pytest --collect-only -q` and asserts that exactly a pinned set of packages contributed collected tests. A wrapper (`py/scripts/run_tests.sh`) supplies `set -euo pipefail`, forwards passthrough args to pytest, and runs the guard only on an unfiltered invocation. `py:test` stays a Moon `command:` so passthrough survives. Independently, one ini line promotes `PytestConfigWarning` to an error, hardening the total-loss case.

**Tech Stack:** Python 3.12.13 (stdlib only), pytest 9.1.1, uv 0.11.16, Moon 2.5.3, bash.

**Spec:** `docs/superpowers/specs/2026-09-03-sma-610-py-test-floor-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0`.
- Conventional commits with a workspace scope, e.g. `ci(py): …`, `docs(py): …`. Include `(SMA-610)`.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`uv` resolve to the repo-pinned versions.
- The guard is pure stdlib. No new dependency, no change to `py/uv.lock`.
- The guard is ruff-linted (`line-length = 200`, rule set `E,F,W,I,N,UP,B,A,C4,SIM,TCH,RUF`) but NOT type-checked — `tool.basedpyright.include` does not reach `py/scripts`. Same posture as `py/scripts/assert_typecheck_coverage.py`.
- Do NOT add a `repo:*` gate, and do NOT touch `ci/affected-graph/ci_targets.py`, `moon.yml`, or `ci/actionlint/run.sh`. The invocation pin was considered and declined (spec, "Placement" + residual 1).
- ~~Do NOT "fix" `py:typecheck`'s identical missing `pipefail`.~~ **Amended mid-execution:** the
  reviewer directed it be folded into this PR, so `.moon/tasks/python.yml`'s `typecheck` now
  carries `set -euo pipefail` too. See the spec's amended out-of-scope entry.
- Run every measurement from `py/` unless stated otherwise. Restore any moved directory before finishing a step; `git status --short` must be empty.

---

### Task 1: Promote `PytestConfigWarning` to an error

**Files:**
- Modify: `py/pyproject.toml` (the `[tool.pytest.ini_options]` table, currently at :28-33)

**Interfaces:**
- Consumes: nothing.
- Produces: total loss of ALL `packages/*/tests` now exits 1 at config time instead of 5. Task 3's wrapper relies on this: with `set -e`, the total-loss case aborts at the real pytest run and never reaches the guard.

- [ ] **Step 1: Measure the current total-loss behaviour**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py
mv packages/paigasus-kernel/tests /tmp/k-tests && mv packages/paigasus-proto/tests /tmp/p-tests
uv run pytest -q; echo "EXIT=$?"
mv /tmp/k-tests packages/paigasus-kernel/tests && mv /tmp/p-tests packages/paigasus-proto/tests
```

Expected: `EXIT=5`, with a `PytestConfigWarning: No files were found in testpaths` in the output. Record it.

- [ ] **Step 2: Add the ini line**

In `py/pyproject.toml`, inside `[tool.pytest.ini_options]`, directly after the `testpaths` line:

```toml
testpaths = ["packages/*/tests"]
# Promote pytest's config-time warnings to errors so a TOTAL loss of packages/*/tests fails
# loudly at config time, naming testpaths, instead of exiting 5 because the recursive fallback
# happened to find nothing (SMA-610 M3 -> M4). This does NOT cover PARTIAL loss -- pytest emits
# no warning at all when the glob still matches something -- which is what
# scripts/assert_test_floor.py exists for.
#
# This promotes SEVEN emission sites in pytest 9.1.1, not one (config/__init__.py:563, :1431,
# :1506, :1613, :2058, :2065, :2235/:2244). Two carry real collateral and are accepted
# deliberately: :1506 makes `Unknown config option: X` a hard error for the whole py workspace
# AND the scoped paigasus-kernel-py:test, and :2065 turns a self-skipping third-party plugin
# into a hard failure. A pytest bump re-opens this measurement.
filterwarnings = ["error::pytest.PytestConfigWarning"]
```

- [ ] **Step 3: Verify total loss is now a hard error**

```bash
cd py
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
mv packages/paigasus-kernel/tests /tmp/k-tests && mv packages/paigasus-proto/tests /tmp/p-tests
uv run pytest -q; echo "EXIT=$?"
mv /tmp/k-tests packages/paigasus-kernel/tests && mv /tmp/p-tests packages/paigasus-proto/tests
```

Expected: `EXIT=1`, output ending in `pytest.PytestConfigWarning: No files were found in testpaths`. Record it.

- [ ] **Step 4: Verify no regression on the intact tree, and that partial loss is STILL green**

```bash
cd py
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run pytest -q | tail -1                       # expect: 134 passed
mv packages/paigasus-kernel/tests /tmp/k-tests
out=$(uv run pytest -q); rc=$?; echo "$out" | tail -1; echo "EXIT=$rc"   # expect: 7 passed, EXIT=0
mv /tmp/k-tests packages/paigasus-kernel/tests
```

Expected: 134 passed; then 7 passed at exit 0. The second result is the point — it proves this task alone does NOT close the issue, and it is why Task 2 exists.

- [ ] **Step 5: Commit**

```bash
git add py/pyproject.toml
git commit -m "test(py): make a total loss of testpaths a hard config error (SMA-610)"
```

---

### Task 2: The collection floor guard

**Files:**
- Create: `py/scripts/assert_test_floor.py`

**Interfaces:**
- Consumes: nothing.
- Produces: `parse_collected(text) -> tuple[set[str], int, int | None]`; `check_stream(text) -> str | None`; `check_registry(disk, tests_dirs) -> str | None`; `check_floor(collected) -> str | None`; the exit constants `EXIT_OK=0`, `EXIT_FLOOR=1`, `EXIT_UNREADABLE=2`, `EXIT_REGISTRY=3`, `EXIT_NO_PACKAGES=4`; and the CLI contract "reads `pytest --collect-only -q` on stdin, or `--self-test`". Task 3 pipes into this script.

- [ ] **Step 1: Write the guard with its built-in fixture suite**

Create `py/scripts/assert_test_floor.py` with exactly this content:

```python
# SPDX-License-Identifier: Apache-2.0
"""Per-package collection floor for the py:test gate (SMA-610).

pytest expands each `testpaths` entry with `glob.iglob` and CONCATENATES the results
(`_pytest/config/__init__.py:1411-1438`, read against the pinned pytest 9.1.1). So if ONE
package's tests/ directory is moved, renamed or deleted, the survivors still collect, pytest
exits 0, and no warning is emitted at all -- measured, 134 passed becomes 7 passed. Nothing
else in the repo sees this: `assert_typecheck_coverage.py` derives BOTH of its numbers from
disk, so a deleted directory drops them together and it stays green.

This guard reads `pytest --collect-only -q` from stdin and fails unless EXACTLY the pinned set
of packages contributed collected tests. Intended use (pipe), with py/ as the cwd:

    uv run pytest --collect-only -q | uv run python scripts/assert_test_floor.py

The pure functions are exercised by a built-in fixture suite, which is also the only way to
reach exit codes 3 and 4 -- they read git, not stdin:

    uv run python scripts/assert_test_floor.py --self-test
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

# Packages that MUST contribute at least one collected test. Compared by STRICT EQUALITY against
# the set actually collected, which makes one comparison bidirectional: a lost tests/ directory
# drops a package out and reds, and a package that gains tests without being added here also reds.
EXPECTED_TEST_PACKAGES = frozenset({"paigasus-kernel", "paigasus-proto"})

# Packages with no tests, each with a reason. Blank reasons are rejected.
#
# A package may NOT be moved from EXPECTED_TEST_PACKAGES to here to silence the floor:
# check_registry() also requires that every package listed here has NO tracked tests/ directory,
# so the reclassification would have to delete a tracked directory as well -- a reviewable event
# rather than a one-line edit that looks like bookkeeping.
NO_TESTS_EXPECTED = {
    "paigasus-ml": "stub package, no public API yet (README 'Status: stub'; ADR-0011 dormant-until-real)",
    "paigasus-workflows": "stub package, no public API yet (README 'Status: stub'; ADR-0011 dormant-until-real)",
}

EXIT_OK = 0
EXIT_FLOOR = 1
EXIT_UNREADABLE = 2
EXIT_REGISTRY = 3
EXIT_NO_PACKAGES = 4

# A collected item's node id. The `::` is LOAD-BEARING. pytest also emits
# `packages/<pkg>/tests/<file>.py:<lineno>` lines -- from its warnings summary
# (`_pytest/terminal.py:367-375`, reported by default under --collect-only because ExitCode.OK is
# in summary_exit_codes) and from collection-error tracebacks. Three such shapes were measured and
# none contains `::`. Without this separator, a package whose test functions were all renamed to
# `check_*` would still be credited for any warning naming a file inside it -- the very defect this
# guard exists to catch, reached by a rename instead of a move. Do not relax this to a prefix match.
NODE_ID_RE = re.compile(r"^packages/([^/]+)/tests/[^:]+\.py::")

# "134 tests collected in 0.04s", "7 tests collected in 0.03s", "1 test collected in 0.01s".
COLLECTED_RE = re.compile(r"^(\d+) tests? collected")
# "no tests collected (134 deselected) in 0.06s" -- only reachable with -k/-m, which the wrapper
# never passes. Parsed as zero rather than treated as a missing summary, so the two stay distinct.
NONE_COLLECTED_RE = re.compile(r"^no tests collected")


def parse_collected(text: str) -> tuple[set[str], int, int | None]:
    """Return (packages, node_id_count, reported_count) from `pytest --collect-only -q` output.

    reported_count is None when pytest printed no collection summary at all -- a crashed or
    truncated producer, which the caller must treat as unreadable rather than as zero.
    """
    packages: set[str] = set()
    node_ids = 0
    reported: int | None = None
    for line in text.splitlines():
        match = NODE_ID_RE.match(line)
        if match:
            packages.add(match.group(1))
            node_ids += 1
            continue
        match = COLLECTED_RE.match(line)
        if match:
            reported = int(match.group(1))
            continue
        if NONE_COLLECTED_RE.match(line):
            reported = 0
    return packages, node_ids, reported


def check_stream(text: str) -> str | None:
    """Integrity of the collect-only stream. Returns an error message, or None if sound."""
    _packages, node_ids, reported = parse_collected(text)
    if reported is None:
        return (
            "py:test floor: `pytest --collect-only -q` printed no collection summary line. The "
            "producer crashed, was killed mid-stream, or its output format changed under the "
            "pytest<10 pin. Refusing to pass on an unreadable stream (SMA-610)."
        )
    if node_ids != reported:
        return (
            f"py:test floor: parsed {node_ids} node id(s) but pytest reported {reported} "
            f"collected. The stream is truncated, or the node-id format changed under the "
            f"pytest<10 pin. Refusing to pass on a partial stream (SMA-610)."
        )
    return None


def check_registry(disk: set[str], tests_dirs: set[str]) -> str | None:
    """The two pinned tables against the tracked tree. Returns an error message, or None."""
    pinned = set(EXPECTED_TEST_PACKAGES)
    listed = set(NO_TESTS_EXPECTED)

    both = sorted(pinned & listed)
    if both:
        return (
            f"py:test floor: {', '.join(both)} appear(s) in BOTH EXPECTED_TEST_PACKAGES and "
            f"NO_TESTS_EXPECTED. A package must be exactly one of the two (SMA-610)."
        )

    blank = sorted(name for name, reason in NO_TESTS_EXPECTED.items() if not reason.strip())
    if blank:
        return (
            f"py:test floor: NO_TESTS_EXPECTED entries {', '.join(blank)} carry a blank reason. "
            f"Every exemption needs a stated reason (SMA-610)."
        )

    missing = sorted(disk - (pinned | listed))
    if missing:
        return (
            f"py:test floor: package(s) {', '.join(missing)} exist under py/packages but appear "
            f"in neither EXPECTED_TEST_PACKAGES nor NO_TESTS_EXPECTED. Classify each one "
            f"deliberately: add it to the floor, or exempt it with a reason (SMA-610)."
        )

    stale = sorted((pinned | listed) - disk)
    if stale:
        return (
            f"py:test floor: package(s) {', '.join(stale)} are pinned but have no tracked "
            f"py/packages/<name>/pyproject.toml. Remove the stale entry (SMA-610)."
        )

    undertested = sorted(pinned - tests_dirs)
    if undertested:
        return (
            f"py:test floor: package(s) {', '.join(undertested)} are in EXPECTED_TEST_PACKAGES "
            f"but have no tracked tests/ directory. If the tests were removed on purpose, that "
            f"is the edit under review (SMA-610)."
        )

    unexpected = sorted(listed & tests_dirs)
    if unexpected:
        return (
            f"py:test floor: package(s) {', '.join(unexpected)} are exempted in "
            f"NO_TESTS_EXPECTED but DO have a tracked tests/ directory. Move them to "
            f"EXPECTED_TEST_PACKAGES so their tests are floored (SMA-610)."
        )

    return None


def check_root(disk: set[str]) -> str | None:
    """Sanity of the tree the guard is reading. Returns an error message, or None.

    Separate from check_registry so that "the guard ran from the wrong place" cannot be
    misreported as "the pins disagree with the tree" -- `assert_typecheck_coverage.py` carries the
    same idea as EXIT_NO_PACKAGES. Split out as a function rather than inlined in main() so the
    self-test can reach it; it is otherwise the one exit code no fixture could demonstrate.
    """
    if not disk:
        return (
            "py:test floor: found no tracked py/packages/*/pyproject.toml. The guard ran from the "
            "wrong directory, git is unavailable, or the packages/* layout moved. Refusing to "
            "pass vacuously (SMA-610)."
        )
    return None


def check_floor(collected: set[str]) -> str | None:
    """The floor itself. Returns an error message, or None if it holds."""
    if collected == set(EXPECTED_TEST_PACKAGES):
        return None
    lost = sorted(set(EXPECTED_TEST_PACKAGES) - collected)
    extra = sorted(collected - set(EXPECTED_TEST_PACKAGES))
    parts: list[str] = []
    if lost:
        parts.append(
            f"contributed NO collected tests: {', '.join(lost)} -- its tests/ directory was "
            f"moved, renamed or emptied, and pytest silently collected only the survivors"
        )
    if extra:
        parts.append(
            f"contributed tests but is not pinned: {', '.join(extra)} -- add it to "
            f"EXPECTED_TEST_PACKAGES"
        )
    return f"py:test floor: {'; '.join(parts)} (SMA-610)."


def tracked_packages(py_root: Path) -> tuple[set[str], set[str]]:
    """Read the tracked tree via git. Returns (packages, packages with a tests/ directory).

    git rather than the filesystem: an untracked scratch package must not red locally while CI
    is green. `ci/affected-graph/task_inputs.py` chooses the tracked set for the same reason.
    """
    result = subprocess.run(
        ["git", "ls-files", "--", "packages"],
        cwd=py_root,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return set(), set()
    disk: set[str] = set()
    tests_dirs: set[str] = set()
    for line in result.stdout.splitlines():
        parts = line.split("/")
        if len(parts) < 3 or parts[0] != "packages":
            continue
        name, rest = parts[1], parts[2:]
        if rest == ["pyproject.toml"]:
            disk.add(name)
        elif rest[0] == "tests" and len(rest) > 1:
            tests_dirs.add(name)
    return disk, tests_dirs


SELF_TEST_STREAM = "\n".join(
    [
        "packages/paigasus-kernel/tests/test_parity.py::test_one",
        "packages/paigasus-proto/tests/test_health_smoke.py::test_two",
        "",
        "2 tests collected in 0.01s",
    ]
)


def self_test() -> int:
    """Drive the pure functions with fixtures. Prints one line per case; returns 0 iff all pass."""
    disk = {"paigasus-kernel", "paigasus-proto", "paigasus-ml", "paigasus-workflows"}
    tests = {"paigasus-kernel", "paigasus-proto"}
    warn_line = "packages/paigasus-kernel/tests/test_parity.py:2"
    trace_line = "packages/paigasus-kernel/tests/test_parity.py:1: in <module>"

    cases: list[tuple[str, bool]] = [
        ("intact stream is sound", check_stream(SELF_TEST_STREAM) is None),
        ("intact stream yields both packages", parse_collected(SELF_TEST_STREAM)[0] == tests),
        ("empty stdin is unreadable", check_stream("") is not None),
        ("summary-only stream is a count mismatch", check_stream("5 tests collected in 0.1s") is not None),
        ("node ids without a summary are unreadable", check_stream(SELF_TEST_STREAM.split("\n")[0]) is not None),
        ("no-tests-collected parses as zero", parse_collected("no tests collected (134 deselected) in 0.06s")[2] == 0),
        ("a warning-summary line is not a node id", parse_collected(warn_line)[0] == set()),
        ("a traceback line is not a node id", parse_collected(trace_line)[0] == set()),
        ("root sanity holds on the real tree", check_root(disk) is None),
        ("an empty tree reds root sanity", check_root(set()) is not None),
        ("registry holds on the real tree", check_registry(disk, tests) is None),
        ("an unclassified package reds", check_registry(disk | {"paigasus-new"}, tests) is not None),
        ("a stale pin reds", check_registry(disk - {"paigasus-ml"}, tests) is not None),
        ("a pinned package with no tests dir reds", check_registry(disk, tests - {"paigasus-kernel"}) is not None),
        ("an exempt package with a tests dir reds", check_registry(disk, tests | {"paigasus-ml"}) is not None),
        ("floor holds for the pinned set", check_floor(tests) is None),
        ("a lost package reds the floor", check_floor({"paigasus-proto"}) is not None),
        ("an unpinned contributor reds the floor", check_floor(tests | {"paigasus-ml"}) is not None),
        ("an empty collection reds the floor", check_floor(set()) is not None),
    ]

    failed = 0
    for name, ok in cases:
        print(f"{'PASS' if ok else 'FAIL'}  {name}")
        failed += 0 if ok else 1
    print(f"\n{len(cases) - failed}/{len(cases)} self-test cases passed")
    return EXIT_OK if failed == 0 else EXIT_FLOOR


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()

    py_root = Path(__file__).resolve().parents[1]
    disk, tests_dirs = tracked_packages(py_root)

    error = check_root(disk)
    if error:
        print(error, file=sys.stderr)
        return EXIT_NO_PACKAGES

    text = sys.stdin.read()

    error = check_stream(text)
    if error:
        print(error, file=sys.stderr)
        return EXIT_UNREADABLE

    error = check_registry(disk, tests_dirs)
    if error:
        print(error, file=sys.stderr)
        return EXIT_REGISTRY

    error = check_floor(parse_collected(text)[0])
    if error:
        print(error, file=sys.stderr)
        return EXIT_FLOOR

    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
```

- [ ] **Step 2: Run the self-test and verify every case passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py
uv run python scripts/assert_test_floor.py --self-test; echo "EXIT=$?"
```

Expected: 19 `PASS` lines, `19/19 self-test cases passed`, `EXIT=0`. If any case FAILs, fix the guard — not the fixture.

- [ ] **Step 3: Demonstrate every exit code**

```bash
cd py
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run pytest --collect-only -q | uv run python scripts/assert_test_floor.py; echo "0? EXIT=$?"
printf '' | uv run python scripts/assert_test_floor.py; echo "2? EXIT=$?"
printf 'packages/paigasus-kernel/tests/t.py::a\n\n1 test collected in 0.0s\n' \
  | uv run python scripts/assert_test_floor.py; echo "1? EXIT=$?"
printf 'x\n\n9 tests collected in 0.0s\n' | uv run python scripts/assert_test_floor.py; echo "2? EXIT=$?"
R="$(git rev-parse --show-toplevel)"
# The venv python directly, NOT `uv run`: from /tmp there is no .prototools, so proto cannot
# resolve uv and dies with `proto::detect::failed` -- an exit 1 that looks like the guard reding
# (measured during execution; it briefly fooled the implementer).
cd /tmp && printf 'packages/paigasus-kernel/tests/t.py::a\n\n1 test collected in 0.0s\n' \
  | "$R/py/.venv/bin/python" "$R/py/scripts/assert_test_floor.py"; echo "cwd-independent? EXIT=$?"
```

Expected in order: `EXIT=0`; `EXIT=2`; `EXIT=1` naming `paigasus-proto` as having contributed no tests; `EXIT=2` (count mismatch); `EXIT=1` from `/tmp` — the guard resolves its root from `__file__`, not the cwd, so it still finds the packages and still reds the floor. Record all outputs.

Exit codes **3** and **4** are covered by the self-test rather than by stdin: both read the tracked tree via git, so no stdin fixture can reach them. `check_root` and `check_registry` are split out as functions for exactly that reason.

- [ ] **Step 4: Lint and format**

```bash
cd py
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run ruff format scripts/assert_test_floor.py
uv run ruff check scripts/assert_test_floor.py
```

Expected: `All checks passed!`. Re-run Step 2 if `ruff format` changed the file.

- [ ] **Step 5: Commit**

```bash
git add py/scripts/assert_test_floor.py
git commit -m "ci(py): add a per-package pytest collection floor (SMA-610)"
```

---

### Task 3: The wrapper and the Moon wiring

**Files:**
- Create: `py/scripts/run_tests.sh`
- Modify: `.moon/tasks/python.yml` (the `lint`, `fmt` and `test` tasks, currently at :25-42)

**Interfaces:**
- Consumes: `py/scripts/assert_test_floor.py` from Task 2, invoked as `uv run python scripts/assert_test_floor.py` with `pytest --collect-only -q` piped in; and Task 1's `filterwarnings`, which makes the total-loss case abort at the first pytest call under `set -e`.
- Produces: `py:test` as `command: 'bash scripts/run_tests.sh'`. Later tasks rely on `moon run py:test` reding on partial loss and on passthrough still working.

- [ ] **Step 1: Create the wrapper**

Create `py/scripts/run_tests.sh` with exactly this content:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# py:test — the suite, then the per-package collection floor (SMA-610).
#
# `set -euo pipefail` is REQUIRED and is the reason this is a script rather than two commands
# chained in the Moon task. Moon does not enable errexit for `script:` blocks and a pipeline's
# status is its LAST command's (moon.yml:68-74 documents the same trap). Without `pipefail` the
# collect-only run's exit status is discarded and the guard's status alone decides — and that is
# reachable, not theoretical: `_validate_config_options` runs in the pytest_collection
# hookwrapper's `finally` (config/__init__.py:1440-1447, 1462-1464), AFTER
# pytest_collection_finish has already printed every node id (terminal.py:905-919). Under the
# filterwarnings promotion added by this same issue, run 2 can emit a complete node-id list and
# still exit non-zero.
set -euo pipefail

uv run pytest "$@"

# The floor only makes sense over an UNFILTERED collection: `moon run py:test -- -k parity`
# legitimately collects from one package. This is the design's one deliberate no-op branch, and
# nothing gates it (SMA-610 residual 1/7) — CI never passes args, so a permanent `args:` entry on
# the Moon task is the edit a reviewer has to catch.
if [ "$#" -eq 0 ]; then
  uv run pytest --collect-only -q | uv run python scripts/assert_test_floor.py
fi
```

- [ ] **Step 2: Verify the wrapper directly, before touching Moon**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd py
bash scripts/run_tests.sh; echo "intact EXIT=$?"
bash scripts/run_tests.sh -k parity; echo "filtered EXIT=$?"
mv packages/paigasus-kernel/tests /tmp/k-tests
bash scripts/run_tests.sh; echo "partial-loss EXIT=$?"
mv /tmp/k-tests packages/paigasus-kernel/tests
```

Expected: intact `EXIT=0` (134 passed); filtered `EXIT=0` with deselections and NO floor output; partial-loss `EXIT=1` with the floor naming `paigasus-kernel`. **The third result is the acceptance criterion for the whole issue** — record its full output.

- [ ] **Step 3: Rewrite the three Moon tasks**

In `.moon/tasks/python.yml`, replace the `lint`, `fmt` and `test` task bodies with:

```yaml
  lint:
    command: 'uv run ruff check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**']
  fmt:
    command: 'uv run ruff format --check .'
    inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**']
```

and, for `test`:

```yaml
  test:
    # `command` (not `script`) deliberately: Moon forwards `moon run py:test -- -k foo` to a
    # command, and SILENTLY DROPS it for a script — measured, `-k parity` gave 124 passed/10
    # deselected as a command and 134 passed as a script. A filtered run that quietly executes
    # the whole suite and reports success is the same silent lie SMA-610 exists to close, so the
    # two steps live in run_tests.sh (which also supplies the `set -euo pipefail` a `script:`
    # block would not have).
    command: 'bash scripts/run_tests.sh'
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'pyproject.toml'
      - '/py/uv.lock'
      # The guard and the wrapper: without this, editing either would not re-run the task and a
      # broken guard would serve a cached pass.
      - 'scripts/**'
      # assert_test_floor.py's registry assertion reads these, so a PR adding a scaffold package
      # that has only a pyproject.toml must select this task — otherwise the assertion never runs
      # on the PR that breaks it and reds an unrelated PR later.
      - 'packages/*/pyproject.toml'
```

Note the `'conftest.py'` entry is **removed**: SMA-379 deleted `py/conftest.py` and the input
declaration outlived the file — a dead input, invisible because `repo:input-liveness` reaches only
`repo:*` tasks (`ci/affected-graph/task_inputs.py`'s `_repo_tasks`).

- [ ] **Step 4: Verify through Moon**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd "$(git rev-parse --show-toplevel)"
moon run py:test --force; echo "intact EXIT=$?"
out=$(moon run py:test --force -- -k parity 2>&1); rc=$?; echo "$out" | grep -E 'deselected|passed'; echo "EXIT=$rc"
cd py && mv packages/paigasus-kernel/tests /tmp/k-tests && cd ..
moon run py:test --force; echo "partial-loss EXIT=$?"
cd py && mv /tmp/k-tests packages/paigasus-kernel/tests && cd ..
moon run paigasus-kernel-py:test --force; echo "scoped EXIT=$?"
```

Expected: intact `EXIT=0`; filtered shows `10 deselected` (passthrough survived); partial-loss non-zero with the floor message; scoped `EXIT=0` (the kernel task is unaffected).

- [ ] **Step 5: Verify the `scripts/**` input actually re-runs the task**

```bash
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run py:test            # warm; expect a cached/fast pass
touch py/scripts/assert_test_floor.py
moon run py:test            # must RE-RUN, not report a cache hit
```

Expected: the second run executes the task rather than serving a cached result. If it caches, the `scripts/**` glob is wrong — fix it before committing.

- [ ] **Step 6: Verify lint/fmt now select on a scripts edit**

```bash
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks --affected --json 2>/dev/null | head -1 >/dev/null   # warm the graph
touch py/scripts/assert_test_floor.py
moon run py:lint py:fmt --force; echo "EXIT=$?"
```

Expected: `EXIT=0`, both tasks run. (Selection itself is proven by the input glob added in Step 3; this step proves the tasks pass over the new files.)

- [ ] **Step 7: Commit**

```bash
git add py/scripts/run_tests.sh .moon/tasks/python.yml
git commit -m "ci(py): run the collection floor from a pipefail wrapper (SMA-610)"
```

---

### Task 4: Documentation and the final measurement table

**Files:**
- Modify: `py/README.md` (the Commands table at :25-31 and the Notes list at :33-39)

**Interfaces:**
- Consumes: everything from Tasks 1-3.
- Produces: nothing downstream.

- [ ] **Step 1: Fix the stale task name in the commands table**

In `py/README.md`, the Format check row currently reads `moon run py:format`. The task is named
`fmt` (`.moon/tasks/python.yml:28`). Change it to:

```markdown
| Format check | `moon run py:fmt` |
```

- [ ] **Step 2: Document the floor**

Add to the Notes list in `py/README.md`, after the `uv sync --package <name>` bullet:

```markdown
- `py:test` runs the suite and then a **per-package collection floor**
  (`scripts/assert_test_floor.py`). `testpaths = ["packages/*/tests"]` is glob-expanded and
  concatenated, so losing ONE package's `tests/` directory leaves pytest collecting the survivors
  at exit 0 with no warning — measured, 134 passed silently became 7 passed. The floor pins which
  packages must contribute tests (`EXPECTED_TEST_PACKAGES`) and which are exempt with a reason
  (`NO_TESTS_EXPECTED`); the two are compared by strict equality against what pytest actually
  collected, so **adding a package with tests, or removing a package's tests, is a deliberate edit
  to that file**. Run `uv run python scripts/assert_test_floor.py --self-test` to exercise it.
- The floor is skipped when you pass arguments through (`moon run py:test -- -k parity`), since a
  filtered run legitimately collects from one package.
```

- [ ] **Step 3: Re-derive the full measurement table on the fixed tree**

Run each row and record the output in the PR body. `WITH` means Task 1's `filterwarnings` and
Tasks 2-3's floor are both in place, i.e. the tree as it now stands.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd "$(git rev-parse --show-toplevel)/py"
echo "--- intact ---";        bash scripts/run_tests.sh >/dev/null 2>&1; echo "EXIT=$?"
mv packages/paigasus-kernel/tests /tmp/k
echo "--- kernel lost ---";   out=$(bash scripts/run_tests.sh 2>&1); rc=$?; echo "$out" | tail -2; echo "EXIT=$rc"
mv /tmp/k packages/paigasus-kernel/tests
mv packages/paigasus-proto/tests /tmp/p
echo "--- proto lost ---";    out=$(bash scripts/run_tests.sh 2>&1); rc=$?; echo "$out" | tail -2; echo "EXIT=$rc"
mv /tmp/p packages/paigasus-proto/tests
mv packages/paigasus-kernel/tests /tmp/k && mv packages/paigasus-proto/tests /tmp/p
echo "--- total loss ---";    out=$(bash scripts/run_tests.sh 2>&1); rc=$?; echo "$out" | tail -2; echo "EXIT=$rc"
mv /tmp/k packages/paigasus-kernel/tests && mv /tmp/p packages/paigasus-proto/tests
cd .. && git status --short
```

Expected: intact 0; kernel lost non-zero via the floor naming `paigasus-kernel`; proto lost
non-zero via the floor naming `paigasus-proto` (a different shape — 127 tests survive rather than
7); total loss non-zero at the FIRST pytest call, via Task 1's config error, before the floor runs.
`git status --short` must be empty.

- [ ] **Step 4: Run the full py gate set**

```bash
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run py:lint py:fmt py:typecheck py:test --force; echo "EXIT=$?"
```

Expected: `EXIT=0`.

- [ ] **Step 5: Commit**

```bash
git add py/README.md
git commit -m "docs(py): document the per-package collection floor (SMA-610)"
```

---

## Notes for the implementer

<!-- moon-diagnosis:ok -->
- **A passing re-run destroys evidence.** If a Moon task fails unexpectedly, copy
  `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` out of the repo BEFORE
  re-running — see CLAUDE.md's `moon-diagnosis` block, which is the authority here. This note
  deliberately does not restate the procedure: it points at it, so the two cannot drift.
- **`py/scripts/run_tests.sh` is not shellchecked by anything.** `repo:actionlint`'s shellcheck
  integration covers workflow `run:` blocks only, not repo shell scripts. Read the wrapper
  carefully rather than relying on a gate to catch a quoting bug.
- If `uv` output ever arrives as JSON inside an agent session, that is the documented proto NDJSON
  behaviour — `export PROTO_REPORTER=text`. It cannot cause a false green here (extra lines do not
  remove node ids, and the count cross-check would red), but it can cause a confusing false red.
