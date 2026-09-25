#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-670 — the self-test for the console image checks in ci/images/run.sh.
#
# Each check that SMA-670 added to assert_console_pins and smoke_consoles gets a surgical mutation
# here that must red with that check's own named ::error:: substring, and the clean baseline must
# stay green. .github/workflows/images.yml runs it directly before "Build + smoke both consoles".
# It is NOT a Moon task, for the reason in the header of ci/images/run.sh.
#
# How it loads the code under test: awk copies each function named in FUNCS out of run.sh, from
# its `<name>() {` line to the next line that is exactly `}`. The copy goes through `bash -n`, is
# sourced, and each name must then be a defined function. run.sh itself is never sourced: its
# dispatch code at the end exits before any function could run. ROOT is the only global that a
# copied function reads, and the harness sets it inside each row's subshell.
#
# How it calls the code under test: in its production shape. assert_console_pins runs as a plain
# command under `set -euo pipefail`, as the run.sh dispatch runs it. A smoke-row function runs as
# `fn … || rc=$?`, as smoke_consoles runs it, so errexit is off inside it. Each row runs in a
# subshell, so an `exit` or a `set -u` failure is a FAIL row and does not stop the harness.
#
# Bash 3.2 (macOS /bin/bash) and bash 5 both run it: no mapfile, no declare -A, no here-string, no
# pipe into an early-exit reader, no expansion of an array that can be empty, no ${v,,}, no [[ -v ]].
#
# Output: one PASS, FAIL or SKIP line for each row. On a FAIL the captured output follows, every
# line prefixed with `    | `, so no output line starts with `::` and the Actions runner makes no
# false annotation. Exit 1 on any FAIL, 2 on an infrastructure error in the loader.
set -euo pipefail
export PROTO_REPORTER=text

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
RUN_SH="$HERE/run.sh"

# The functions copied out of run.sh. A task that adds a function to run.sh adds its name here.
FUNCS="assert_console_pins with_deadline"

T="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-console-selftest.XXXXXX")"
HC_CTR="selftest-hc-$$"
cleanup() {
  if command -v docker >/dev/null 2>&1; then
    docker rm -f "$HC_CTR" >/dev/null 2>&1 || true
  fi
  rm -rf "$T"
}
trap cleanup EXIT

N_PASS=0
N_FAIL=0
N_SKIP=0

infra() {
  echo "console-selftest: infrastructure error (rc=2): $*" >&2
  exit 2
}

say_pass() {
  echo "PASS $1"
  N_PASS=$((N_PASS + 1))
}

say_skip() {
  echo "SKIP $1 — $2"
  N_SKIP=$((N_SKIP + 1))
}

# say_fail <row> <reason> [<file>...] — every line of each non-empty file follows, prefixed.
say_fail() {
  local row="$1" why="$2" f
  shift 2
  echo "FAIL ${row} — ${why}"
  for f in "$@"; do
    [ -s "$f" ] || continue
    echo "    | --- ${f##*/}"
    sed 's/^/    | /' "$f"
  done
  N_FAIL=$((N_FAIL + 1))
}

# check_row <row> <rc> <want_rc> <present> <absent> <out> <err> — a row passes only when the rc is
# the expected one, every `|`-separated substring of <present> is in stderr, and <absent> (when
# not empty) is not. `case` matches the fixed substrings: no grep, so no regex and no early-exit
# reader.
check_row() {
  local row="$1" rc="$2" want_rc="$3" present="$4" absent="$5" o="$6" e="$7" err one rest
  err="$(cat "$e")"
  if [ "$rc" -ne "$want_rc" ]; then
    say_fail "$row" "rc ${rc}, expected ${want_rc}" "$o" "$e"
    return 0
  fi
  rest="$present"
  while [ -n "$rest" ]; do
    one="${rest%%|*}"
    if [ "$one" = "$rest" ]; then rest=""; else rest="${rest#*|}"; fi
    case "$err" in
      *"$one"*) ;;
      *) say_fail "$row" "stderr does not hold '${one}'" "$o" "$e"; return 0 ;;
    esac
  done
  if [ -n "$absent" ]; then
    case "$err" in
      *"$absent"*) say_fail "$row" "stderr holds '${absent}', which it must not" "$o" "$e"; return 0 ;;
    esac
  fi
  say_pass "$row"
}

# --- loader ------------------------------------------------------------------------------------
extract_fn() {
  awk -v want="$1() {" '$0 == want { on = 1 } on { print } on && $0 == "}" { on = 0 }' "$RUN_SH"
}

: > "$T/funcs.sh"
for fn in $FUNCS; do
  extract_fn "$fn" > "$T/fn-$fn.sh" || infra "awk could not read ${RUN_SH}"
  [ -s "$T/fn-$fn.sh" ] || infra "no '${fn}() {' line in ci/images/run.sh; the loader copies from that line to the next line that is exactly '}'"
  cat "$T/fn-$fn.sh" >> "$T/funcs.sh"
done
"$BASH" -n "$T/funcs.sh" || infra "bash -n rejected the functions copied out of ci/images/run.sh"
# shellcheck source=/dev/null
. "$T/funcs.sh"
for fn in $FUNCS; do
  declare -F "$fn" >/dev/null || infra "${fn} is not a defined function after the copy was sourced"
done

# --- static rows (assert_console_pins) ---------------------------------------------------------
# run_pins <row> <root> <want_rc> <present> — the production shape: a plain command under
# `set -euo pipefail`, with errexit ON inside the function.
run_pins() {
  local row="$1" root="$2" want_rc="$3" present="$4" rc o e
  o="$T/$row.out"
  e="$T/$row.err"
  set +e
  # shellcheck disable=SC2034 # ROOT is read by the function under test.
  ( ROOT="$root"; set -euo pipefail; assert_console_pins ) >"$o" 2>"$e"
  rc=$?
  set -e
  check_row "$row" "$rc" "$want_rc" "$present" "" "$o" "$e"
}

# mk_tree <row> — a fixture ROOT that holds a copy of the real ts/Dockerfile and .prototools.
mk_tree() {
  mkdir -p "$T/$1/ts"
  cp "$REPO/ts/Dockerfile" "$T/$1/ts/Dockerfile"
  cp "$REPO/.prototools" "$T/$1/.prototools"
}

# append_df <row> <line>... — one append of one or more lines to the fixture Dockerfile.
append_df() {
  local row="$1"
  shift
  printf '%s\n' "$@" >> "$T/$row/ts/Dockerfile"
}

# sed_df <row> <sed-script> — one sed over the fixture Dockerfile. No `sed -i`: BSD and GNU sed
# take its argument differently.
sed_df() {
  sed "$2" "$T/$1/ts/Dockerfile" > "$T/$1/ts/Dockerfile.new"
  mv "$T/$1/ts/Dockerfile.new" "$T/$1/ts/Dockerfile"
}

# pins_mut <row> <want_rc> <present> — a mutation row. The mutation must have changed the copy,
# or the row is a FAIL named "mutation did not apply".
pins_mut() {
  if cmp -s "$REPO/ts/Dockerfile" "$T/$1/ts/Dockerfile"; then
    say_fail "$1" "mutation did not apply"
    return 0
  fi
  run_pins "$1" "$T/$1" "$2" "$3"
}

run_pins S0 "$REPO" 0 ""

mk_tree C2
append_df C2 '# PAIGASUS_X in a comment'
pins_mut C2 0 ""

mk_tree S1
append_df S1 'FROM node:latest AS extra'
pins_mut S1 1 'FROM instruction(s)'

mk_tree S1b
append_df S1b 'from node:latest as extra'
pins_mut S1b 1 'FROM instruction(s)'

mk_tree S1c
awk '/^FROM node:/ && !done { print "RUN true \\"; done = 1 } { print }' "$T/S1c/ts/Dockerfile" > "$T/S1c/ts/Dockerfile.new"
mv "$T/S1c/ts/Dockerfile.new" "$T/S1c/ts/Dockerfile"
pins_mut S1c 1 'FROM instruction(s)'

mk_tree S1d
{ printf '%s\n' '# syntax=docker/dockerfile:1'; cat "$REPO/ts/Dockerfile"; } > "$T/S1d/ts/Dockerfile"
pins_mut S1d 1 'parser directive'

mk_tree S1g
{ printf '%s\n' '#  SYNTAX = docker/dockerfile:1'; cat "$REPO/ts/Dockerfile"; } > "$T/S1g/ts/Dockerfile"
pins_mut S1g 1 'parser directive'

mk_tree S1e
append_df S1e 'COPY --from=alpine:latest /x /y'
pins_mut S1e 1 'which is not the builder stage'

mk_tree S1f
append_df S1f 'RUN --mount=type=bind,from=alpine:latest,target=/x true'
pins_mut S1f 1 'which is not the builder stage'

mk_tree S2
append_df S2 'RUN pnpm i --filter x'
pins_mut S2 1 'carry --frozen-lockfile'

mk_tree S2b
append_df S2b 'RUN pnpm i'
pins_mut S2b 1 'carry --frozen-lockfile'

mk_tree S2c
append_df S2c 'RUN pnpm --filter x install'
pins_mut S2c 1 'carry --frozen-lockfile'

mk_tree S2d
append_df S2d 'RUN pnpm i --frozen-lockfile && pnpm i'
pins_mut S2d 1 'carry --frozen-lockfile'

mk_tree S2e
append_df S2e 'RUN sh -c "pnpm install"'
pins_mut S2e 1 'carry --frozen-lockfile'

mk_tree C1
append_df C1 'RUN pnpm info x && pnpm import && pnpm init'
pins_mut C1 0 ""

# --- summary -----------------------------------------------------------------------------------
echo "console-selftest: ${N_PASS} passed, ${N_FAIL} failed, ${N_SKIP} skipped"
if [ "$N_FAIL" -ne 0 ]; then
  exit 1
fi
if [ "$N_SKIP" -ne 0 ] && [ "${CI:-}" = "true" ]; then
  echo "console-selftest: rows were skipped with CI=true; in CI a skipped row is a failure"
  exit 1
fi
exit 0
