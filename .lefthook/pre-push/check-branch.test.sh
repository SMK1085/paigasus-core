#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# -e intentionally omitted so assert() can capture non-zero exit codes from $sut.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
sut="$here/check-branch.sh"
fail=0

assert() { # $1 desc  $2 want_exit  $3 branch
  err=$("$sut" "$3" 2>&1 >/dev/null); code=$?
  if [ "$code" -ne "$2" ]; then
    printf 'FAIL: %s (branch=%s got=%d want=%d)\n  stderr: %s\n' "$1" "$3" "$code" "$2" "$err"
    fail=1
  else
    echo "ok:   $1"
  fi
}

assert "feature/ passes"        0 "feature/sma-371-local-git-hooks"
assert "main allow-listed"      0 "main"
assert "dependabot allow-listed" 0 "dependabot/npm_and_yarn/commitlint-19"
assert "sven/ rejected"         1 "sven/foo"
assert "uppercase rejected"     1 "feature/SMA-371"
assert "bare slug rejected"     1 "wip"
assert "empty/detached skipped" 0 ""

exit "$fail"
