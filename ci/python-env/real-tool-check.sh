#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Runs the cheap operations of ci/python-env/run.sh against the real tool. A step in
# repo:affected-smoke runs it on every PR, so an operation that first runs on main is proven on the
# PR. The lock operation is not called: it refuses under moon by design.
# Usage: bash ci/python-env/real-tool-check.sh. Exit codes: 1 wrong result, 2 usage error.
set -euo pipefail
export PROTO_REPORTER=text

[ "$#" -eq 0 ] || {
  printf 'real-tool-check: takes no argument\n' >&2
  exit 2
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
WRAP="$ROOT/ci/python-env/run.sh"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/real-tool-check.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

fail() {
  printf 'real-tool-check: FAIL: %s\n' "$*" >&2
  exit 1
}

bash "$WRAP" probe || fail "probe: the tool is not on PATH"

dir="$(bash "$WRAP" cache-dir)" || fail "cache-dir failed"
case "$dir" in
  /* | [A-Za-z]:*) ;;
  *) fail "cache-dir is not an absolute path: $dir" ;;
esac
[ "$(printf '%s\n' "$dir" | wc -l | tr -d ' ')" = 1 ] || fail "cache-dir prints more than one line"

(cd "$ROOT/py" && bash "$WRAP" run py python -c pass) || fail "run py python -c pass failed"

printf 'import sys\nsys.exit(int(sys.argv[1]))\n' >"$tmp/ok.py"
rc=0
bash "$WRAP" script "$tmp/ok.py" 3 || rc=$?
[ "$rc" -eq 3 ] || fail "script returned $rc, want 3"

mkdir "$tmp/dist"
bash "$WRAP" build paigasus-proto "$tmp/dist" >/dev/null 2>&1 || fail "build paigasus-proto failed"
n_tar="$(find "$tmp/dist" -maxdepth 1 -name '*.tar.gz' | wc -l | tr -d ' ')"
n_whl="$(find "$tmp/dist" -maxdepth 1 -name '*.whl' | wc -l | tr -d ' ')"
[ "$n_tar" = 1 ] && [ "$n_whl" = 1 ] || fail "build made $n_tar sdist and $n_whl wheel, want 1 and 1"

printf 'real-tool-check: all operations passed\n'
