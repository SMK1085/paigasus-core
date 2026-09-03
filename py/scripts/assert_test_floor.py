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


def check_registry(disk: set[str], tests_dirs: set[str]) -> str | None:
    """The two pinned tables against the tracked tree. Returns an error message, or None."""
    pinned = set(EXPECTED_TEST_PACKAGES)
    listed = set(NO_TESTS_EXPECTED)

    both = sorted(pinned & listed)
    if both:
        return f"py:test floor: {', '.join(both)} appear(s) in BOTH EXPECTED_TEST_PACKAGES and NO_TESTS_EXPECTED. A package must be exactly one of the two (SMA-610)."

    blank = sorted(name for name, reason in NO_TESTS_EXPECTED.items() if not reason.strip())
    if blank:
        return f"py:test floor: NO_TESTS_EXPECTED entries {', '.join(blank)} carry a blank reason. Every exemption needs a stated reason (SMA-610)."

    missing = sorted(disk - (pinned | listed))
    if missing:
        return (
            f"py:test floor: package(s) {', '.join(missing)} exist under py/packages but appear "
            f"in neither EXPECTED_TEST_PACKAGES nor NO_TESTS_EXPECTED. Classify each one "
            f"deliberately: add it to the floor, or exempt it with a reason (SMA-610)."
        )

    stale = sorted((pinned | listed) - disk)
    if stale:
        return f"py:test floor: package(s) {', '.join(stale)} are pinned but have no tracked py/packages/<name>/pyproject.toml. Remove the stale entry (SMA-610)."

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


def check_floor(collected: set[str]) -> str | None:
    """The floor itself. Returns an error message, or None if it holds."""
    if collected == set(EXPECTED_TEST_PACKAGES):
        return None
    lost = sorted(set(EXPECTED_TEST_PACKAGES) - collected)
    extra = sorted(collected - set(EXPECTED_TEST_PACKAGES))
    parts: list[str] = []
    if lost:
        parts.append(f"contributed NO collected tests: {', '.join(lost)} -- its tests/ directory was moved, renamed or emptied, and pytest silently collected only the survivors")
    if extra:
        parts.append(f"contributed tests but is not pinned: {', '.join(extra)} -- add it to EXPECTED_TEST_PACKAGES")
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

    # Fail fast rather than block. The guard is a filter and its stdin is normally a pipe, but
    # invoked bare from a terminal `sys.stdin.read()` waits for EOF forever -- a hang is a worse
    # failure than a red, and an agent or CI step that reaches this by mistake would stall rather
    # than report (measured: still waiting after 5s).
    if sys.stdin.isatty():
        print(
            "py:test floor: stdin is a terminal. This guard reads `pytest --collect-only -q` from "
            "a pipe; run it as `uv run pytest --collect-only -q | uv run python "
            "scripts/assert_test_floor.py`, or use --self-test (SMA-610).",
            file=sys.stderr,
        )
        return EXIT_UNREADABLE

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
