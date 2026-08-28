#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:workflow-credentials — assert no pull-request-triggered workflow can obtain a
# repository credential. Same-repo pull requests receive repository secrets, so a credential
# in such a workflow is readable by any code the pull request introduces (SMA-407 §7 M2).
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed.
#
# The checker exits 3, not 1, for an assertion failure. `uv` exits 1 on its own failures —
# MEASURED on a failed resolution both online and with UV_OFFLINE=1 — so without a distinct
# code a PyPI outage would report "a workflow declares a credential". This wrapper owns the
# translation and nothing else may.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/workflow-credentials"

die_infra() { printf 'workflow-credentials: %s\n' "$*" >&2; exit 2; }

# Preflight. `uv` absent yields 127 from the shell, which is neither 0/1/2 nor actionable.
command -v uv >/dev/null 2>&1 \
  || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"

# $@ is forwarded to the checker. Returns 0, returns 1 for a real assertion failure, and
# EXITS 2 for anything else.
run_checker() {
  local rc=0
  uv run --project "$HERE" --python '>=3.12' python3 \
    "$HERE/workflow_credentials.py" "$@" || rc=$?
  case "$rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;
  esac
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        MODE=selftest; shift ;;
    --negative-control) MODE=negctl;   shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_checker --self-test ;;
  check)    run_checker "$REPO_ROOT" ;;
  negctl)   negative_control ;;
esac
