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
  # PYTEST_ADDOPTS is cleared deliberately. The floor's question is "does every pinned package
  # still contribute tests", which is about the TREE, not about whatever filter a developer has
  # exported into their shell. Left set, `PYTEST_ADDOPTS=-k foo` filters the collection too and
  # the floor reds on a tree that is perfectly intact (measured: exit 2). Clearing it here keeps
  # the real run above honouring the developer's filter while the floor stays authoritative.
  # Explicitly '' rather than a bare `PYTEST_ADDOPTS=`: shellcheck 0.11.0 (the version this repo
  # pins for repo:actionlint) reports SC1007 on the bare form, since it cannot tell a deliberate
  # empty assignment from a typo'd one. Same semantics, no warning.
  PYTEST_ADDOPTS='' uv run pytest --collect-only -q | uv run python scripts/assert_test_floor.py
fi
