#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SMA-409 / SMA-429 — affected-graph regression guard (strict-equality, default-deny).
#
# `moon ci` USES the affected graph but never ASSERTS it is correct, so a deleted
# dependsOn edge (or a dropped `moon ci --include-relations`) silently under-builds and
# stays green. This guard feeds a synthetic touched-file to `moon query projects
# --affected --downstream deep` and asserts the resulting project set EQUALS a known
# expected set per case (default-deny — any unlisted project present fails the case), so
# such a regression fails red. See
# docs/superpowers/specs/2026-06-14-sma-409-affected-graph-cascade-guard-design.md (guard
# architecture) and
# docs/superpowers/specs/2026-06-16-sma-429-affected-graph-completeness-guard-design.md
# (strict-equality model).
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

# assert_task_case LABEL FILE EXPECTED_CSV
#   Same strict-equality contract as assert_case, but over the TASK graph: the set of `build`,
#   `test` and `lint` targets scheduled by the touched file must EQUAL the expected set.
#
#   Why a second query: `moon query projects --affected` follows `dependsOn` ONLY and is structurally
#   blind to a task-level `^:build` (SMA-429 F3). Delete the `^:build` from a moon.yml and every
#   project case above stays GREEN while `moon ci --include-relations` silently under-builds — the
#   exact hole SMA-524 exists to close. This case sees it.
#
#   Scoped to build/test/lint — the three tasks that carry `^:build` (lint joined them in
#   SMA-526). fmt and build-release are excluded because they carry no `^:build`: fmt is
#   crate-local by construction, and build-release does not run in CI at all.
#
#   NOTE: the filter matches task NAMES across every project, not just Rust ones, so a
#   same-named task in another stack could enter a case's observed set. `contracts:lint` exists
#   and does not appear here — contracts is UPSTREAM of paigasus-proto-rs and `--downstream deep`
#   walks dependents — but a future case with a different touched file must re-check that.
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_task_case() {
  local label="$1" file="$2" expected_csv="$3" got want missing unexpected
  [ -n "$expected_csv" ] || { echo "FATAL [$label]: EXPECTED_CSV is empty (harness bug)" >&2; return 2; }
  got="$(printf '%s\n' "$file" \
    | moon query tasks --affected --downstream deep \
    | python3 -c '
import sys, json
d = json.load(sys.stdin)
out = []
for pid, tasks in (d.get("tasks") or {}).items():
    for name in tasks:
        if name in ("build", "test", "lint"):
            out.append(f"{pid}:{name}")
print("\n".join(sorted(out)))')" \
    || { echo "FATAL [$label]: moon query tasks failed" >&2; return 2; }
  want="$(tr ',' '\n' <<<"$expected_csv" | sort)"
  if [ "$got" = "$want" ]; then
    printf 'PASS  %-18s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
    return 0
  fi
  missing="$(comm -23 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  unexpected="$(comm -13 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  echo "FAIL  [$label] affected TASK set != expected set" >&2
  if [ -n "$missing" ]; then
    echo "  missing  (expected but not scheduled — likely a dropped task-level '^:build'; for" >&2
    echo "  \`lint\` that dep lives once in .moon/tasks/rust.yml, not per-crate):" >&2
    sed 's/^/    /' <<<"$missing" >&2
  fi
  if [ -n "$unexpected" ]; then
    echo "  unexpected (scheduled but not expected — if the new edge is intended, add it here):" >&2
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

# Task-graph twin of run_case — same 3-way return-code folding.
run_task_case() {
  local ec=0
  assert_task_case "$@" || ec=$?
  case "$ec" in
    0) ;;
    1) SUITE_RC=1 ;;
    *) echo "== affected-graph guard ABORTED: infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}

# Generic Cargo<->Moon parity gate. rc 2 (infra) aborts, mirroring run_case.
assert_cargo_moon_parity() {
  local ec=0
  python3 "$HERE/cargo_moon_parity.py" || ec=$?
  case "$ec" in
    0) return 0 ;;
    1) return 1 ;;
    *) echo "== affected-graph guard ABORTED: parity gate infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}

# SMA-541 — CI target-array coverage. rc 2 (infra) aborts, mirroring run_case.
assert_ci_targets() {
  local ec=0
  python3 "$HERE/ci_targets.py" || ec=$?
  case "$ec" in
    0) return 0 ;;
    1) return 1 ;;
    *) echo "== affected-graph guard ABORTED: ci-targets infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}

run_suite() {
  SUITE_RC=0
  # contracts proto edit -> proto packages in all three languages + the gateway rebuild + the
  # IAM service crate that consumes paigasus-proto-rs for its gRPC surface (SMA-442) + the
  # shared descriptor crate that consumes the generated ServiceInfo/Capability types (SMA-505).
  run_case "contracts->proto" "contracts/proto/paigasus/gateway/v1/health.proto" \
    "contracts,paigasus-proto-rs,paigasus-proto-py,paigasus-proto-ts,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs"
  # derive-crate edit -> the derive crate + paigasus-proto and everything downstream of it
  # (SMA-438). One-directional w.r.t. contracts: the derive crate is strictly UPSTREAM of
  # paigasus-proto, so a proto edit must NOT reach it — enforced implicitly by the strict
  # equality of the contracts->proto case above, which lists no derive crate.
  # paigasus-service-info-rs is here via its paigasus-proto edge, wired in SMA-524.
  run_case "proto-derive->proto" "rs/crates/libs/paigasus-proto-derive/src/lib.rs" \
    "paigasus-proto-derive-rs,paigasus-proto-rs,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs"
  # service-info edit -> the crate + both services that serve the descriptor (SMA-524). Guards the
  # DOWNSTREAM direction, which no case covered before: paigasus-service-info was a graph LEAF, so an
  # edit to ServiceInfoDto — the wire body both services return — retested nothing.
  # One-directional: paigasus-proto-rs is deliberately absent (a consumer edit must not rebuild the
  # contract crate), enforced implicitly by strict equality.
  run_case "service-info->services" "rs/crates/libs/paigasus-service-info/src/lib.rs" \
    "paigasus-service-info-rs,paigasus-iam-rs,paigasus-gateway-rs"
  # kernel edit -> kernel + all three bindings (py/node/wasm) + gateway + both language wrappers (SMA-419/420/427)
  # + the IAM crates that consume the kernel's PRN/UUIDv7 (paigasus-iam-core-rs & the paigasus-iam-rs
  # service, SMA-441). paigasus-logging-rs is deliberately ABSENT — it has no kernel edge.
  # + paigasus-observability-rs, whose correlation layer mints UUIDv7 via the kernel (SMA-504).
  # Strict equality (default-deny): any OTHER project appearing (an unrelated *-py/*-ts package, a
  # contracts/py/ts root) fails the case automatically — no forbid enumeration needed.
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts,paigasus-wasm-rs,paigasus-kernel-parity-rs,paigasus-iam-core-rs,paigasus-iam-rs,paigasus-observability-rs"
  # py binding edit -> the binding + the py wrapper that depends on it (SMA-419). One-directional
  # w.r.t. the kernel: paigasus-kernel-rs is deliberately ABSENT (a binding edit must not rebuild
  # the kernel), now enforced implicitly by strict equality rather than a forbid-regex.
  run_case "binding-oneway"   "rs/crates/bindings/paigasus-py-bindings/src/lib.rs" \
    "paigasus-py-bindings-rs,paigasus-kernel-py"
  # node binding edit -> the node binding + the ts wrapper that depends on it (SMA-420). Likewise
  # one-directional: paigasus-kernel-rs deliberately absent.
  run_case "binding-oneway-node" "rs/crates/bindings/paigasus-node-bindings/src/lib.rs" \
    "paigasus-node-bindings-rs,paigasus-kernel-ts"
  # wasm binding edit -> the wasm binding + the ts wrapper that depends on it (SMA-427). Likewise
  # one-directional: paigasus-kernel-rs deliberately absent (a binding edit must not rebuild the kernel).
  run_case "binding-oneway-wasm" "rs/crates/bindings/paigasus-wasm/src/lib.rs" \
    "paigasus-wasm-rs,paigasus-kernel-ts"
  # parity crate edit -> only the parity crate. One-directional w.r.t. the kernel: a parity edit
  # must NOT rebuild the kernel (paigasus-kernel-rs deliberately absent), now enforced implicitly by
  # strict equality. Confirms Moon treats the cross-project corpus `inputs` of the py/ts test tasks
  # as task-hash keys, NOT as project-affected edges (so py/ts do not appear here) — SMA-433.
  run_case "parity-oneway" "rs/crates/libs/paigasus-kernel-parity/src/lib.rs" \
    "paigasus-kernel-parity-rs"
  # A proto edit must SCHEDULE paigasus-service-info's build, test AND lint, not merely mark the
  # project affected. This is the behavioral half of SMA-524 (build/test) and SMA-526 (lint): the
  # parity gate asserts `^:build` is DECLARED, this asserts it takes EFFECT.
  run_task_case "proto->service-info-tasks" "rs/crates/libs/paigasus-proto/src/lib.rs" \
    "paigasus-proto-rs:build,paigasus-proto-rs:test,paigasus-proto-rs:lint,paigasus-service-info-rs:build,paigasus-service-info-rs:test,paigasus-service-info-rs:lint,paigasus-iam-rs:build,paigasus-iam-rs:test,paigasus-iam-rs:lint,paigasus-gateway-rs:build,paigasus-gateway-rs:test,paigasus-gateway-rs:lint"
  # A workspace-level change must schedule EVERY crate's lint, AND the three tasks that compile the
  # FFI cdylibs. `rs/` has no Moon project, so these files belong to `repo`; affectedness reaches
  # both sets through task INPUTS, not through `dependsOn` — which is why no project case above
  # changes. Before SMA-534 a Cargo.lock-only touch (i.e. every Dependabot Cargo PR) scheduled no
  # crate task at all, so a dependency bump that tripped `-D warnings` merged green and redded main
  # later.
  #
  # The three build/test rows are SMA-546. `cargo clippy` emits metadata and never LINKS, and runs
  # on the host target only — so the thirteen lints cannot cover the three `crate-type = ["cdylib"]`
  # bindings, for which linking IS the failure mode, nor wasm32-unknown-unknown, which they never
  # compile. paigasus-kernel-ts:{build,test} and paigasus-kernel-py:test are the tasks that do.
  #
  # The case name still says `all-lint`. It is now a slight misnomer, kept deliberately: it is
  # referenced by CLAUDE.md and ci/affected-graph/README.md, and renaming it would break those
  # greps for no functional gain.
  #
  # SAFETY OF THE NAME FILTER: `assert_task_case` matches the task NAMES build/test/lint across
  # every project, so a same-named task elsewhere would enter this set. One premise makes that safe
  # and it must be stated narrowly: `repo` declares no task named build/test/lint (verify:
  # `moon query tasks`). The py/ts side is no longer a premise but an ASSERTION — the three tasks
  # that key on `rs/Cargo.lock` are listed below, so a fourth one appearing shows up here as an
  # `unexpected` row rather than passing silently. Add it if intended; do not widen the filter.
  #
  # The py CONFIGURATION ROOT's tasks (py:test/lint/fmt/typecheck) are deliberately absent. They do
  # not key on these files: `uv run` alone serves a CACHED wheel and cannot observe a Rust change
  # (measured for SMA-546 — a kernel edit that made `--reinstall-package` fail 67 tests left plain
  # `uv run pytest` reporting 124 passed), so giving them these inputs would buy cost with no
  # coverage.
  run_task_case "lockfile->all-lint" "rs/Cargo.lock" \
    "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:lint,paigasus-kernel-py:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-logging-rs:lint,paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,paigasus-wasm-rs:lint"
  # Generic Cargo<->Moon parity: catches a MISSING case, which is how SMA-524's bug survived review.
  assert_cargo_moon_parity || SUITE_RC=1
  # assert_include_relations returns only 0/1 (no infra code), so collapsing is correct here.
  assert_include_relations || SUITE_RC=1
  # LAST deliberately: assert_ci_targets is the only assertion that can still exit 2 (a broken
  # `moon query`), and an rc-2 abort kills the script — so anything ordered after it would lose
  # its diagnostics on exactly the runs where they are most useful (SMA-541 D2).
  assert_ci_targets || SUITE_RC=1
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
  # 3) the parity gate must fire on synthetic violations of each of its three assertions — a gate
  #    that can pass vacuously reproduces the very bug it exists to prevent (SMA-524 D6).
  python3 "$HERE/cargo_moon_parity.py" --self-test || NEG_RC=1
  # 4) the ci-target coverage gate must fire on synthetic violations of each of its four checks —
  #    including its two hand-rolled parsers, which are the part it cannot self-detect a fault in.
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
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
