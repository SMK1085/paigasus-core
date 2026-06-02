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
    --ecosystem) ECOSYSTEM="$2"; shift 2 ;;
    --negative-control) NEGATIVE=1; shift ;;
    -h|--help) echo "usage: run.sh [--ecosystem NAME] [--negative-control]"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# shellcheck source=ci/release-parity/ecosystems/release-plz.sh
source "$HERE/ecosystems/$ECOSYSTEM.sh"

REPO_ROOT="$(cd "$HERE/../.." && pwd)"
REAL_TOML="$REPO_ROOT/rs/release-plz.toml"
BASELINE="0.1.0"
CASES="$HERE/cases.tsv"

# returns 0 iff crate a bumps to $expected AND crate b stays at baseline.
check_case() { # id subject footer expected
  local id="$1" subject="$2" footer="$3" expected="$4" dir got_a got_b
  dir="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$dir'" RETURN
  ecosystem::build_fixture "$dir" "$REAL_TOML"
  ecosystem::apply_commit "$dir" a "$subject" "$footer"
  ecosystem::run_update "$dir"
  got_a="$(ecosystem::version "$dir" a)"
  got_b="$(ecosystem::version "$dir" b)"
  if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]; then
    printf 'PASS  %-12s a=%s b=%s\n' "$id" "$got_a" "$got_b"
    return 0
  fi
  printf 'FAIL  %-12s a:exp=%s got=%s | b:exp=%s got=%s\n' \
    "$id" "$expected" "$got_a" "$BASELINE" "$got_b" >&2
  return 1
}

if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: feeding a deliberately wrong expectation =="
  if check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1"; then
    echo "negative-control FAILED: harness accepted a wrong expectation" >&2
    exit 1
  fi
  echo "negative-control OK: harness reported red as expected"
  exit 0
fi

rc=0
while IFS=$'\t' read -r id subject footer expected_0x _expected_1x _discr; do
  case "$id" in ''|'#'*) continue ;; esac
  check_case "$id" "$subject" "$footer" "$expected_0x" || rc=1
done <"$CASES"

if [ "$rc" = 0 ]; then echo "== all parity cases passed =="; else echo "== parity FAILURES (see above) ==" >&2; fi
exit "$rc"
