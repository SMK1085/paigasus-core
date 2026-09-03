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
