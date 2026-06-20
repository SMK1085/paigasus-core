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
# No /** suffix here (unlike the pyproject include): expected_count() rglobs each matched dir
# itself, so these intentionally terminate at the directory. Don't "align" them by adding /**.
INCLUDE_GLOBS = ("packages/*/src", "packages/*/tests")
EXCLUDE_PARTS = frozenset({"generated", "__pycache__", "node_modules", ".venv", "dist", "build"})

EXIT_OK = 0
EXIT_UNDER_FLOOR = 1
EXIT_UNREADABLE = 2
EXIT_NO_PACKAGES = 3


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
    if expected == 0:
        print(
            "py:typecheck coverage guard: computed an expected source-file count of 0 -- the py/packages/* layout may have moved or the guard's root is wrong. Refusing to pass vacuously (SMA-436).",
            file=sys.stderr,
        )
        return EXIT_NO_PACKAGES

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
