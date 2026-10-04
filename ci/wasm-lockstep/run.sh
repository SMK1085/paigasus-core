#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:wasm-lockstep — the gate over .github/workflows/wasm-lockstep.yml and its checker (SMA-693).
#
#   run.sh                     pin_check.py on the real workflow, and lockstep_check.py current on
#                              HEAD's rs/Cargo.lock (the family invariant every weekly run needs)
#   run.sh --self-test         both checkers' in-process fixture tables, plus this wrapper's rc rows
#   run.sh --negative-control  both checkers' controls on the real inputs, plus the 3 -> 1 mapping
#                              on a real refusal
#
# Exit codes: 0 pass | 1 an assertion failed | 2 infrastructure error. Both checkers exit 3 for an
# assertion (and lockstep_check.py 4 for "no change"). This wrapper maps 3 to 1 and every other
# non-zero code to 2, because `uv` and a Python traceback both exit 1 (ci/CLAUDE.md). Do not
# "normalize" a checker to 1.
#
# The lock comes from `git show HEAD:rs/Cargo.lock`, never from the working tree: an unlocked
# cargo call inside `moon ci` can rewrite the working-tree lock during the run (spec F15).
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no pipe into
# an early-exit reader (ci/actionlint check 13). A function called in `|| rc=$?` position runs
# with errexit OFF, so every function below routes each status by hand and never calls
# die_infra from inside a command substitution (the errexit-swallows-nested-exit trap).
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment, which poisons every $(...) capture of a
# proto or shim call. Exported once here, so every later capture inherits it (SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/wasm-lockstep"
WORKFLOW="$REPO_ROOT/.github/workflows/wasm-lockstep.yml"

die_infra() { printf 'wasm-lockstep: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        MODE=selftest; shift ;;
    --negative-control) MODE=negctl;   shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

TMP="$(mktemp -d)" || die_infra "mktemp -d failed"
trap 'rm -rf "$TMP"' EXIT

# The venv interpreter, resolved ONCE, at the top level. --locked stops uv from rewriting
# uv.lock. lockstep_check.py needs only the stdlib, but runs through the same >=3.12 interpreter,
# because the host python3 of the development Mac can be older than tomllib (3.11).
command -v uv >/dev/null 2>&1 || die_infra "uv is not on PATH; run 'proto install', or add ~/.proto/shims to PATH"
PY="$(uv run --locked --project "$HERE" --python '>=3.12' python3 -c 'import sys, tomllib, yaml; print(sys.executable)')" \
  || die_infra "uv could not provide the locked PyYAML environment for $HERE"
case "$PY" in
  "$HERE/.venv/"*) ;;
  *) die_infra "the interpreter is not under $HERE/.venv. Got: ${PY:-<empty>}" ;;
esac
git -C "$REPO_ROOT" show HEAD:rs/Cargo.lock >"$TMP/head.lock" || die_infra "git show HEAD:rs/Cargo.lock failed"

# A checker's raw rc -> the gate's rc. Echo only; never exits.
gate_rc() {
  case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac
}

# Runs "$PY" "$@" and RETURNS the mapped rc. Never exits.
run_checker() {
  local raw=0
  "$PY" "$@" || raw=$?
  return "$(gate_rc "$raw")"
}

report() {  # $1 label, $2 gate rc
  case "$2" in
    0) printf 'PASS  [%s]\n' "$1" ;;
    1) printf 'FAIL  [%s]: assertion failure\n' "$1" >&2 ;;
    *) printf 'FAIL  [%s]: infrastructure error (rc=2)\n' "$1" >&2 ;;
  esac
}

real_run() {
  local worst=0 pin_rc lock_rc
  pin_rc=0; run_checker "$HERE/pin_check.py" "$WORKFLOW" || pin_rc=$?
  report "pin_check $WORKFLOW" "$pin_rc"
  if [ "$pin_rc" -gt "$worst" ]; then worst="$pin_rc"; fi
  lock_rc=0; run_checker "$HERE/lockstep_check.py" current --lock "$TMP/head.lock" || lock_rc=$?
  report "lockstep_check current on HEAD:rs/Cargo.lock" "$lock_rc"
  if [ "$lock_rc" -gt "$worst" ]; then worst="$lock_rc"; fi
  exit "$worst"
}

self_test() {
  local st_worst=0 lst_rc pst_rc pair raw want got map_rc
  lst_rc=0; run_checker "$HERE/lockstep_check.py" --self-test || lst_rc=$?
  report "lockstep_check --self-test" "$lst_rc"
  if [ "$lst_rc" -gt "$st_worst" ]; then st_worst="$lst_rc"; fi
  pst_rc=0; run_checker "$HERE/pin_check.py" --self-test || pst_rc=$?
  report "pin_check --self-test" "$pst_rc"
  if [ "$pst_rc" -gt "$st_worst" ]; then st_worst="$pst_rc"; fi
  # The mapping table, and the mapping of a REAL child status (a table alone does not prove that
  # run_checker routes the status it gets).
  for pair in 0:0 3:1 1:2 2:2 4:2 127:2; do
    raw="${pair%%:*}"; want="${pair#*:}"
    got="$(gate_rc "$raw")"
    if [ "$got" != "$want" ]; then
      printf 'FAIL  [gate_rc %s]: got %s, want %s\n' "$raw" "$got" "$want" >&2
      [ "$st_worst" -ge 1 ] || st_worst=1
    fi
    map_rc=0; run_checker -c "raise SystemExit($raw)" || map_rc=$?
    if [ "$map_rc" != "$want" ]; then
      printf 'FAIL  [run_checker exit %s]: got %s, want %s\n' "$raw" "$map_rc" "$want" >&2
      [ "$st_worst" -ge 1 ] || st_worst=1
    fi
  done
  if [ "$st_worst" -ne 0 ]; then
    printf 'wasm-lockstep self-test: FAILED (rc=%s)\n' "$st_worst" >&2
    exit "$st_worst"
  fi
  printf '== wasm-lockstep self-test passed ==\n'
}

negative_control() {
  local nc_worst=0 lnc_rc pnc_rc nc_map_rc
  lnc_rc=0; run_checker "$HERE/lockstep_check.py" --negative-control --lock "$TMP/head.lock" || lnc_rc=$?
  report "lockstep_check --negative-control" "$lnc_rc"
  if [ "$lnc_rc" -gt "$nc_worst" ]; then nc_worst="$lnc_rc"; fi
  pnc_rc=0; run_checker "$HERE/pin_check.py" --negative-control "$WORKFLOW" || pnc_rc=$?
  report "pin_check --negative-control" "$pnc_rc"
  if [ "$pnc_rc" -gt "$nc_worst" ]; then nc_worst="$pnc_rc"; fi
  # The 3 -> 1 mapping on a REAL refusal: HEAD's lock with the version of its first [[package]]
  # changed. awk is the same on BSD and GNU for this script. The refusal must reach this wrapper
  # as 1 and name its code; an infrastructure exit or a pass is a failure of the control.
  awk 'BEGIN { done = 0 } /^version = "/ && !done && seen { print "version = \"999.0.0\""; done = 1; next } /^\[\[package\]\]/ { seen = 1 } { print }' \
    "$TMP/head.lock" >"$TMP/mutated.lock" || die_infra "awk could not write the mutated lock"
  if cmp -s "$TMP/head.lock" "$TMP/mutated.lock"; then
    die_infra "the mutation changed nothing in HEAD's rs/Cargo.lock"
  fi
  nc_map_rc=0; run_checker "$HERE/lockstep_check.py" lock --old "$TMP/head.lock" --new "$TMP/mutated.lock" >"$TMP/map.out" 2>&1 || nc_map_rc=$?
  if [ "$nc_map_rc" -ne 1 ]; then
    printf 'FAIL  [3 -> 1 on a real refusal]: the wrapper returned %s, want 1\n' "$nc_map_rc" >&2
    cat "$TMP/map.out" >&2
    [ "$nc_worst" -ge 1 ] || nc_worst=1
  fi
  if ! grep -qF -- 'R-NONFAMILY' "$TMP/map.out"; then
    printf 'FAIL  [3 -> 1 on a real refusal]: the refusal does not name R-NONFAMILY\n' >&2
    [ "$nc_worst" -ge 1 ] || nc_worst=1
  fi
  if [ "$nc_worst" -ne 0 ]; then
    printf 'wasm-lockstep negative control: FAILED (rc=%s)\n' "$nc_worst" >&2
    exit "$nc_worst"
  fi
  printf '== wasm-lockstep negative control passed ==\n'
}

case "$MODE" in
  selftest) self_test ;;
  negctl)   negative_control ;;
  check)    real_run ;;
esac
