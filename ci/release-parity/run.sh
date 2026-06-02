#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SMA-398 release->semver parity harness (tool-agnostic core).
# usage: run.sh [--ecosystem NAME] [--negative-control]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ECOSYSTEM="release-plz"
NEGATIVE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --ecosystem) [ -n "${2-}" ] || { echo "error: --ecosystem requires a value" >&2; exit 2; }
                 ECOSYSTEM="$2"; shift 2 ;;
    --negative-control) NEGATIVE=1; shift ;;
    -h|--help) echo "usage: run.sh [--ecosystem NAME] [--negative-control]"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# shellcheck source=ci/release-parity/ecosystems/release-plz.sh
source "$HERE/ecosystems/$ECOSYSTEM.sh"

REPO_ROOT="$(cd "$HERE/../.." && pwd)"
REAL_TOML="$REPO_ROOT/rs/release-plz.toml"   # release-plz-specific; other ecosystems may ignore
BASELINE="0.1.0"
CASES="$HERE/cases.tsv"

# returns 0 = pass, 1 = assertion fail, 2 = infrastructure error.
check_case() { # id subject footer expected
  local id="$1" subject="$2" footer="$3" expected="$4" dir got_a got_b
  dir="$(mktemp -d)" || { echo "FATAL [$id]: mktemp failed" >&2; return 2; }
  # shellcheck disable=SC2064
  trap "rm -rf '$dir'" RETURN
  ecosystem::build_fixture "$dir" "$REAL_TOML" || { echo "FATAL [$id]: build_fixture failed" >&2; return 2; }
  ecosystem::apply_commit "$dir" a "$subject" "$footer" || { echo "FATAL [$id]: apply_commit failed" >&2; return 2; }
  ecosystem::run_update "$dir" || { echo "FATAL [$id]: run_update failed" >&2; return 2; }
  got_a="$(ecosystem::version "$dir" a)"
  got_b="$(ecosystem::version "$dir" b)"
  if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]; then
    printf 'PASS  %-12s a=%s b=%s\n' "$id" "$got_a" "$got_b"   # PASS -> stdout
    return 0
  fi
  printf 'FAIL  %-12s a:exp=%s got=%s | b:exp=%s got=%s\n' \
    "$id" "$expected" "$got_a" "$BASELINE" "$got_b" >&2        # FAIL -> stderr
  return 1
}

if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: feeding a deliberately wrong expectation =="
  # fix! in 0.x bumps to 0.2.0 (minor), so 0.1.1 (patch) is deliberately wrong.
  ec=0; check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1" || ec=$?
  case "$ec" in
    1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;
    0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;
    *) echo "negative-control INCONCLUSIVE: infrastructure error (rc=$ec)" >&2; exit 2 ;;
  esac
fi

rc=0
while IFS=$'\t' read -r id subject footer expected_0x _expected_1x _discr || [ -n "$id" ]; do
  case "$id" in ''|'#'*) continue ;; esac
  ec=0; check_case "$id" "$subject" "$footer" "$expected_0x" || ec=$?
  case "$ec" in
    0) ;;
    1) rc=1 ;;
    *) echo "== parity ABORTED: infrastructure error on case $id ==" >&2; exit 2 ;;
  esac
done <"$CASES"

if [ "$rc" = 0 ]; then echo "== all parity cases passed =="; else echo "== parity FAILURES (see above) ==" >&2; fi
exit "$rc"
