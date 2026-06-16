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

# assert_case LABEL FILE EXPECTED_CSV
#   EXPECTED_CSV : comma-separated project ids. The affected set (minus `repo`) must EQUAL this
#                  set exactly — default-deny: any project present that is not listed fails the
#                  case (no separate forbid list; cross-stack isolation is implicit).
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_case() {
  local label="$1" file="$2" expected_csv="$3" got want missing unexpected
  [ -n "$expected_csv" ] || { echo "FATAL [$label]: EXPECTED_CSV is empty (harness bug)" >&2; return 2; }
  got="$(affected_ids "$file")" || { echo "FATAL [$label]: moon query failed" >&2; return 2; }
  # Split the CSV on commas into lines and sort, to match affected_ids' sorted output. Use `tr`,
  # NOT an unquoted `${expected_csv//,/ }` word-split: the latter depends on IFS word-splitting
  # (absent in zsh) and is exposed to globbing — fragile. The expected CSV is hand-written in
  # arbitrary order, so the sort makes the comparison order-insensitive.
  want="$(tr ',' '\n' <<<"$expected_csv" | sort)"
  if [ "$got" = "$want" ]; then
    printf 'PASS  %-18s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
    return 0
  fi
  missing="$(comm -23 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  unexpected="$(comm -13 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  echo "FAIL  [$label] affected set != expected set" >&2
  if [ -n "$missing" ]; then
    echo "  missing  (expected but absent — likely a dropped dependsOn edge or a lost --include-relations):" >&2
    sed 's/^/    /' <<<"$missing" >&2
  fi
  if [ -n "$unexpected" ]; then
    echo "  unexpected (present but not expected — a cross-stack leak/regression, OR a legitimate new" >&2
    echo "  dependent: if the new edge is intended, add it to this case's expected set):" >&2
    sed 's/^/    /' <<<"$unexpected" >&2
  fi
  return 1
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
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs"
  # kernel edit -> kernel + both bindings + gateway + both language wrappers (SMA-419/420).
  # Strict equality (default-deny): any OTHER project appearing (an unrelated *-py/*-ts package, a
  # contracts/py/ts root) fails the case automatically — no forbid enumeration needed.
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts"
  # py binding edit -> the binding + the py wrapper that depends on it (SMA-419). One-directional
  # w.r.t. the kernel: paigasus-kernel-rs is deliberately ABSENT (a binding edit must not rebuild
  # the kernel), now enforced implicitly by strict equality rather than a forbid-regex.
  run_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs,paigasus-kernel-py"
  # node binding edit -> the node binding + the ts wrapper that depends on it (SMA-420). Likewise
  # one-directional: paigasus-kernel-rs deliberately absent.
  run_case "binding-oneway-node" "rs/crates/bindings/paigasus-node-bindings/src/lib.rs" \
    "paigasus-node-bindings-rs,paigasus-kernel-ts"
  # assert_include_relations returns only 0/1 (no infra code), so collapsing is correct here.
  assert_include_relations || SUITE_RC=1
  return "$SUITE_RC"
}

if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: assert deliberately-wrong expectations report red =="
  NEG_RC=0
  # expect_red LABEL FILE EXPECTED_CSV — assert the harness reports a red (rc=1) for a wrong
  # expectation; record a failed control if it green-lights one; abort on infra error (rc=2).
  expect_red() {
    local rc=0
    assert_case "$1" "$2" "$3" || rc=$?
    case "$rc" in
      1) echo "  OK   [$1] harness reported red as expected" ;;
      0) echo "  FAIL [$1] harness accepted a wrong expectation" >&2; NEG_RC=1 ;;
      *) echo "  INCONCLUSIVE [$1] infrastructure error (rc=$rc)" >&2; exit 2 ;;
    esac
  }
  # 1) wrong project: a kernel edit does NOT affect paigasus-proto-py, so requiring it must fail.
  expect_red "neg-wrong-expect"     "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-proto-py"
  # 2) default-deny direction (NEW in SMA-429): an INCOMPLETE expected set must fail on the extras.
  #    Under the old positive-superset model this PASSED (a subset satisfied the must-include check),
  #    silently unasserting every project left out — the exact gap strict equality closes.
  expect_red "neg-incomplete-expect" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-rs"
  if [ "$NEG_RC" = 0 ]; then
    echo "negative-control OK: harness reported red on all wrong expectations"; exit 0
  else
    echo "negative-control FAILED: harness green-lit a wrong expectation" >&2; exit 1
  fi
fi

if run_suite; then
  echo "== affected-graph cascade intact =="
else
  echo "== affected-graph REGRESSION (see FAILs above) ==" >&2
  exit 1
fi
