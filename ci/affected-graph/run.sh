#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SMA-409 — affected-graph regression guard.
#
# `moon ci` USES the affected graph but never ASSERTS it is correct, so a deleted
# dependsOn edge (or a dropped `moon ci --include-relations`) silently under-builds and
# stays green. This guard feeds a synthetic touched-file to `moon query projects
# --affected --downstream deep` and asserts the resulting project set per known case, so
# such a regression fails red. See
# docs/superpowers/specs/2026-06-14-sma-409-affected-graph-cascade-guard-design.md.
#
# usage: run.sh [--negative-control]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
CI_YML="$REPO_ROOT/.github/workflows/ci.yml"
NEGATIVE=0
[ "${1-}" = "--negative-control" ] && NEGATIVE=1

# affected_ids FILE -> newline-sorted project ids, minus `repo` (its source is the repo
# root `.`, so it owns every file and appears for every touch — pure noise here).
affected_ids() { # file
  printf '%s\n' "$1" \
    | moon query projects --affected --downstream deep \
    | python3 -c 'import sys,json; print("\n".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"] != "repo")))'
}

# assert_case LABEL FILE MUST_INCLUDE_CSV FORBID_REGEX
#   MUST_INCLUDE_CSV : comma-separated project ids that MUST be present (positive superset)
#   FORBID_REGEX     : extended regex; any matching id present = cross-stack leak (empty = skip)
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_case() {
  local label="$1" file="$2" inc="$3" forbid="$4" got rc=0 p leaked
  got="$(affected_ids "$file")" || { echo "FATAL [$label]: moon query failed" >&2; return 2; }
  for p in ${inc//,/ }; do
    grep -qx "$p" <<<"$got" || { echo "FAIL  [$label] missing expected project: $p" >&2; rc=1; }
  done
  if [ -n "$forbid" ]; then
    leaked="$(grep -E "$forbid" <<<"$got" || true)"
    [ -z "$leaked" ] || { echo "FAIL  [$label] cross-stack leak: $(tr '\n' ' ' <<<"$leaked")" >&2; rc=1; }
  fi
  [ "$rc" = 0 ] && printf 'PASS  %-18s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
  return "$rc"
}

# Every real `moon ci` shell invocation in ci.yml must carry --include-relations: it is the
# flag that activates relation/dependent rebuilds. The edges are inert without it, so guarding
# the edges but not the flag would leave a hole (SMA-409 review F1). Match only the actual
# command invocations — `moon ci "${T[@]}" ...` — NOT the job/step `name:` fields or the
# comments that also contain the words "moon ci" (matching those would false-FAIL; renaming
# the job away from "moon ci" would also break the `CI / moon ci` required status check).
assert_include_relations() {
  local invocations bad
  invocations="$(grep -nE 'moon ci +"' "$CI_YML" || true)"
  if [ -z "$invocations" ]; then
    echo "FAIL  [ci-include-relations] no 'moon ci \"\${T[@]}\"' invocation found in ci.yml" >&2
    return 1
  fi
  bad="$(printf '%s\n' "$invocations" | grep -v -- '--include-relations' || true)"
  if [ -n "$bad" ]; then
    echo "FAIL  [ci-include-relations] a 'moon ci' invocation lacks --include-relations:" >&2
    printf '%s\n' "$bad" >&2
    return 1
  fi
  printf 'PASS  %-18s -> every `moon ci` invocation carries --include-relations\n' "ci-include-relations"
}

# Run one assert_case and fold its 3-way return code into SUITE_RC: 0 pass, 1 assertion
# failure (record a red suite), 2 infrastructure error (e.g. `moon query` died) -> abort the
# whole guard with exit 2 so a broken `moon` is never mistaken for a graph regression. Mirrors
# the infra-vs-assertion distinction in ci/release-parity/run.sh.
run_case() {
  local ec=0
  assert_case "$@" || ec=$?
  case "$ec" in
    0) ;;
    1) SUITE_RC=1 ;;
    *) echo "== affected-graph guard ABORTED: infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}

run_suite() {
  SUITE_RC=0
  # contracts proto edit -> proto packages in all three languages + the gateway rebuild.
  run_case "contracts->proto" "contracts/proto/paigasus/gateway/v1/health.proto" \
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs" ""
  # kernel edit -> kernel + binding + gateway; nothing cross-stack (no *-py / *-ts / contracts).
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs" \
    '(-py|-ts)$|^contracts$|^py$|^ts$'
  # binding edit -> only the binding; the edge is one-directional (must not drag in the kernel).
  run_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs" '^paigasus-kernel-rs$'
  # assert_include_relations returns only 0/1 (no infra code), so collapsing is correct here.
  assert_include_relations || SUITE_RC=1
  return "$SUITE_RC"
}

if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: assert a deliberately-wrong expectation reports red =="
  # paigasus-kernel-py is NOT a dependent of the kernel crate, so requiring it MUST fail.
  rc=0
  assert_case "neg-wrong-expect" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-py" "" || rc=$?
  case "$rc" in
    1) echo "negative-control OK: harness reported red as expected"; exit 0 ;;
    0) echo "negative-control FAILED: harness accepted a wrong expectation" >&2; exit 1 ;;
    *) echo "negative-control INCONCLUSIVE: infrastructure error (rc=$rc)" >&2; exit 2 ;;
  esac
fi

if run_suite; then
  echo "== affected-graph cascade intact =="
else
  echo "== affected-graph REGRESSION (see FAILs above) ==" >&2
  exit 1
fi
