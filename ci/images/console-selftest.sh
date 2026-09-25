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
FUNCS="assert_console_pins with_deadline console_node_version_row smoke_consoles console_image_config_row console_healthcheck_row"

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
# `set -euo pipefail`, with errexit ON inside the function. PINS_PATH_EXTRA, when set, goes ahead
# of PATH inside the subshell only — the S6g row uses it to put a stub `grep` in front of the real
# one, without touching any other row's PATH.
PINS_PATH_EXTRA=""
run_pins() {
  local row="$1" root="$2" want_rc="$3" present="$4" rc o e
  o="$T/$row.out"
  e="$T/$row.err"
  set +e
  # shellcheck disable=SC2034 # ROOT is read by the function under test.
  ( PATH="${PINS_PATH_EXTRA:+$PINS_PATH_EXTRA:}$PATH"; ROOT="$root"; set -euo pipefail; assert_console_pins ) >"$o" 2>"$e"
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

mk_tree S3
append_df S3 'RUN echo PAIGASUS_X=1 > /app/.env'
pins_mut S3 1 'names PAIGASUS_ outside an ENV/ARG'

mk_tree S4
append_df S4 'COPY --from=builder /x /app/PAIGASUS_X'
pins_mut S4 1 'names PAIGASUS_ outside an ENV/ARG'

mk_tree S5
append_df S5 'RUN <<EOF' 'PAIGASUS_X=1' 'EOF'
pins_mut S5 1 'names PAIGASUS_ outside an ENV/ARG'

mk_tree S6
sed_df S6 's/, { signal: AbortSignal\.timeout(2500) }//'
pins_mut S6 1 'no AbortSignal.timeout'

mk_tree S6b
sed_df S6b 's/AbortSignal\.timeout(2500)/AbortSignal.timeout(3000)/'
pins_mut S6b 1 'no AbortSignal.timeout'

mk_tree S6c
sed_df S6c 's/--timeout=3s/--timeout=3000ms/'
pins_mut S6c 1 'HEALTHCHECK --timeout <none> s'

# S6d (SMA-670 review finding 1): a LEADING ZERO in --timeout must not make bash read the value as
# octal. --timeout=08s is a valid, larger-than-the-signal timeout (8000 ms > the 2500 ms signal),
# so the row must PASS.
mk_tree S6d
sed_df S6d 's/--timeout=3s/--timeout=08s/'
pins_mut S6d 0 ""

# S6e (SMA-670 review finding 1): a --timeout value of more than 5 digits must be refused before
# it is read as an arithmetic operand, so the multiplication cannot overflow.
mk_tree S6e
sed_df S6e 's/--timeout=3s/--timeout=999999s/'
pins_mut S6e 1 'more than 5 digits'

# S6f (SMA-670 review finding 3): exactly one HEALTHCHECK instruction is required. Docker obeys
# only the LAST one; the read above always takes the FIRST, so a second instruction must red
# rather than silently check the wrong timeout.
mk_tree S6f
append_df S6f 'HEALTHCHECK --timeout=1s CMD ["/nodejs/bin/node", "/app/healthcheck.mjs"]'
pins_mut S6f 1 'exactly one is required'

# S6g (SMA-670 review finding 4): a grep rc > 1 (grep itself could not run) inside the hc_to_s
# pipeline must get its own "could not run" ::error::, not fold into "<none>". A stub `grep` ahead
# of the real one on PATH execs the real binary for every pattern except the HEALTHCHECK
# --timeout= one, which it always fails with rc 2 — so every earlier check in assert_console_pins
# still runs normally on the real ts/Dockerfile, and only the pipeline this row targets sees a
# broken grep. Built with `printf`, not a heredoc, for the same reason as the docker stub below.
REAL_GREP="$(command -v grep)" || infra "no system grep on PATH"
mkdir -p "$T/stub-grep-to"
{
  printf '%s\n' '#!/usr/bin/env bash'
  printf '%s\n' 'case "$*" in'
  printf '%s\n' '  *--timeout=*) exit 2 ;;'
  printf '%s\n' 'esac'
  printf '%s\n' "exec '${REAL_GREP}' \"\$@\""
} > "$T/stub-grep-to/grep"
chmod +x "$T/stub-grep-to/grep"
PINS_PATH_EXTRA="$T/stub-grep-to"
run_pins S6g "$REPO" 1 "healthcheck timeout check could not run"
PINS_PATH_EXTRA=""

# S1h (SMA-670 fail-open fix): a grep rc > 1 inside the FROM-pin check (the `-vE` grep that finds
# FROM lines matching neither pin regex) must get its own "could not run" ::error::, not fold into
# "no bad lines". The stub always fails only the `-vE …distroless…` call, so every earlier check
# (which uses `-cE`, not `-vE`, on the same regex) still runs normally.
mkdir -p "$T/stub-grep-s1h"
{
  printf '%s\n' '#!/usr/bin/env bash'
  printf '%s\n' 'case "$*" in'
  printf '%s\n' '  -vE*distroless*) exit 2 ;;'
  printf '%s\n' 'esac'
  printf '%s\n' "exec '${REAL_GREP}' \"\$@\""
} > "$T/stub-grep-s1h/grep"
chmod +x "$T/stub-grep-s1h/grep"
PINS_PATH_EXTRA="$T/stub-grep-s1h"
run_pins S1h "$REPO" 1 "on the FROM lines"
PINS_PATH_EXTRA=""

# S1i (SMA-670 fail-open fix): a grep rc > 1 inside the --from=/--mount= check (the
# `-vxE 'builder|bindings'` grep) must get its own "could not run" ::error::, not fold into "no
# bad value". The stub always fails only that call.
mkdir -p "$T/stub-grep-s1i"
{
  printf '%s\n' '#!/usr/bin/env bash'
  printf '%s\n' 'case "$*" in'
  printf '%s\n' '  -vxE*bindings*) exit 2 ;;'
  printf '%s\n' 'esac'
  printf '%s\n' "exec '${REAL_GREP}' \"\$@\""
} > "$T/stub-grep-s1i/grep"
chmod +x "$T/stub-grep-s1i/grep"
PINS_PATH_EXTRA="$T/stub-grep-s1i"
run_pins S1i "$REPO" 1 "on the --from= values"
PINS_PATH_EXTRA=""

# --- the real fetch timeout (F0, F1) -----------------------------------------------------------
# The rendered healthcheck.mjs runs in the RUNTIME image that the runtime FROM line names, digest
# included: that is the node that runs it in production. A driver starts a server on
# 127.0.0.1:3000 that accepts connections and never answers, and then imports the program from a
# data: URL. F0: with the 2500 ms signal the program must end by itself, rc exactly 1, with
# TimeoutError in its output. F1: with the signal removed it must hang until the watchdog kills
# it (rc 143 or 137). Together they prove that the signal is what ends the hang. with_deadline
# writes to a FILE, never to a `$( )` capture (SMA-670 M7).
HC_DRIVER_JS='
const net = require("net");
const server = net.createServer(() => {});
server.listen(3000, "127.0.0.1", () => {
  import("data:text/javascript," + encodeURIComponent(process.env.HC_SRC));
});
'
HC_RC=0
# hc_run <src-file> <deadline> <out-file> — sets HC_RC.
hc_run() {
  local src
  src="$(cat "$1")"
  docker rm -f "$HC_CTR" >/dev/null 2>&1 || true
  HC_RC=0
  with_deadline "$2" docker run --rm --init --network none --name "$HC_CTR" \
    -e "HC_SRC=${src}" --entrypoint /nodejs/bin/node "$HC_RUNTIME_REF" -e "$HC_DRIVER_JS" >"$3" 2>&1 || HC_RC=$?
  docker rm -f "$HC_CTR" >/dev/null 2>&1 || true
}

HC_RUNTIME_REF=""
if ! docker info >/dev/null 2>&1; then
  say_skip F0 "docker info failed, so no daemon can run the runtime image"
  say_skip F1 "docker info failed, so no daemon can run the runtime image"
else
  hc_line="$(grep -E '^RUN printf .*> /app/healthcheck\.mjs$' "$REPO/ts/Dockerfile")" || hc_line=""
  hc_fmt="${hc_line#*\'}"
  hc_fmt="${hc_fmt%%\'*}"
  HC_RUNTIME_REF="$(grep -oE '^FROM gcr\.io/distroless/nodejs[0-9]+-debian12:nonroot@sha256:[0-9a-f]{64}' "$REPO/ts/Dockerfile" | sed 's/^FROM //')" || HC_RUNTIME_REF=""
  if [ -z "$hc_line" ] || [ "$hc_fmt" = "$hc_line" ]; then
    say_fail F0 "healthcheck printf not found in ts/Dockerfile"
    say_fail F1 "healthcheck printf not found in ts/Dockerfile"
  elif [ -z "$HC_RUNTIME_REF" ]; then
    say_fail F0 "no digest-pinned runtime FROM line in ts/Dockerfile"
    say_fail F1 "no digest-pinned runtime FROM line in ts/Dockerfile"
  elif ! docker pull "$HC_RUNTIME_REF" >"$T/pull.log" 2>&1; then
    say_fail F0 "could not pull the runtime image ${HC_RUNTIME_REF}" "$T/pull.log"
    say_fail F1 "could not pull the runtime image ${HC_RUNTIME_REF}" "$T/pull.log"
  else
    # The format string holds only `\n` and one `%s`, which every printf renders the same way.
    # shellcheck disable=SC2059
    printf "$hc_fmt" /iam > "$T/hc.mjs"
    hc_run "$T/hc.mjs" 15 "$T/F0.out"
    case "$HC_RC" in
      1)
        case "$(cat "$T/F0.out")" in
          *TimeoutError*) say_pass F0 ;;
          *) say_fail F0 "rc 1, but the output holds no TimeoutError" "$T/F0.out" ;;
        esac
        ;;
      143|137) say_fail F0 "the healthcheck hung until the watchdog killed it (rc ${HC_RC})" "$T/F0.out" ;;
      *) say_fail F0 "rc ${HC_RC}, expected exactly 1 with TimeoutError" "$T/F0.out" ;;
    esac
    sed 's/, { signal: AbortSignal\.timeout(2500) }//' "$T/hc.mjs" > "$T/hc-nosignal.mjs"
    if cmp -s "$T/hc.mjs" "$T/hc-nosignal.mjs"; then
      say_fail F1 "mutation did not apply" "$T/hc.mjs"
    else
      hc_run "$T/hc-nosignal.mjs" 6 "$T/F1.out"
      case "$HC_RC" in
        143|137) say_pass F1 ;;
        *) say_fail F1 "rc ${HC_RC}; without the signal the program must hang until the watchdog kills it (143 or 137)" "$T/F1.out" ;;
      esac
    fi
    docker rmi "$HC_RUNTIME_REF" >/dev/null 2>&1 || true
  fi
fi

# --- smoke-row rows (a stub docker) ------------------------------------------------------------
# The stub records its argv (one argument per line, a multi-line argument as <multi-line>, and a
# `--` line after each call) and answers from STUB_* variables. Each row puts the stub first on
# PATH inside its own subshell only, so the F rows above use the real docker. `exec sleep` makes
# the sleep the process that with_deadline kills.
stub_docker_main() {
  local a
  for a in "$@"; do
    case "$a" in *$'\n'*) a="<multi-line>" ;; esac
    printf '%s\n' "$a" >> "$STUB_ARGV"
  done
  printf '%s\n' "--" >> "$STUB_ARGV"
  case "${1:-}" in
    run)
      for a in "$@"; do
        if [ "$a" = "--version" ]; then
          if [ -n "$STUB_VERSION_ERR" ]; then printf '%s\n' "$STUB_VERSION_ERR" >&2; fi
          if [ -n "$STUB_VERSION_OUT" ]; then printf '%s\n' "$STUB_VERSION_OUT"; fi
          exit "$STUB_VERSION_RC"
        fi
      done
      if [ -n "$STUB_WALK_OUT" ]; then printf '%s\n' "$STUB_WALK_OUT"; fi
      exit "$STUB_WALK_RC"
      ;;
    image)
      if [ -n "$STUB_ENV_OUT" ]; then printf '%s\n' "$STUB_ENV_OUT"; fi
      exit "$STUB_ENV_RC"
      ;;
    exec)
      if [ "$STUB_EXEC_SLEEP" -gt 0 ]; then exec sleep "$STUB_EXEC_SLEEP"; fi
      exit "$STUB_EXEC_RC"
      ;;
  esac
  echo "stub docker: unexpected argv: $*" >&2
  exit 99
}
# `declare -f`, not a heredoc: bash 5 writes a heredoc into a pipe before its reader starts, and
# on a host whose new pipe holds only 512 bytes that write can hang (CLAUDE.md, SMA-612).
mkdir -p "$T/stub"
{
  printf '%s\n' '#!/usr/bin/env bash'
  declare -f stub_docker_main
  # shellcheck disable=SC2016 # the line is written into the stub file literally
  printf '%s\n' 'stub_docker_main "$@"'
} > "$T/stub/docker"
chmod +x "$T/stub/docker"

stub_reset() {
  STUB_VERSION_OUT="v24.16.0"; STUB_VERSION_RC=0; STUB_VERSION_ERR=""
  STUB_ENV_OUT="PATH=/usr/bin"; STUB_ENV_RC=0
  STUB_WALK_OUT="walked=1300"; STUB_WALK_RC=0
  STUB_EXEC_SLEEP=0; STUB_EXEC_RC=0
  FX_ROOT="$T/fx-node"
  # A row-scoped extra PATH entry, prepended ahead of the docker stub, empty by default. E6 (the
  # grep-rc-2 row) is the only row that sets it.
  STUB_PATH_EXTRA=""
}

# The fixture .prototools for the N rows: a fixed node pin, so a Node bump in the real
# .prototools does not red this self-test.
mkdir -p "$T/fx-node" "$T/fx-nopin"
printf '%s\n' 'node = "24.16.0"' 'pnpm = "11.3.0"' > "$T/fx-node/.prototools"
printf '%s\n' 'pnpm = "11.3.0"' > "$T/fx-nopin/.prototools"

# SMA-688: each image row takes `<app> <image>` and must hand docker the IMAGE, never a name it
# builds from `<app>`. The rows pass the load-oci name, which differs from `<app>:dev`, so a row
# that tests `${app}:dev` gets a different argv and reds.
T_IMAGE="paigasus-iam-console:dev"

# The argv that console_node_version_row must hand to docker.
printf '%s\n' run --rm --entrypoint /nodejs/bin/node "$T_IMAGE" --version -- > "$T/argv-node"

# run_fn <row> <want_rc> <present> <absent> <want-argv-file|none> <fn> [<arg>...] — the
# production shape: `fn … || rc=$?`, so errexit is off inside the function.
run_fn() {
  local row="$1" want_rc="$2" present="$3" absent="$4" want_argv="$5" rc=0 o e
  shift 5
  o="$T/$row.out"
  e="$T/$row.err"
  rm -f "$T/argv"
  (
    PATH="${STUB_PATH_EXTRA:+$STUB_PATH_EXTRA:}$T/stub:$PATH"
    STUB_ARGV="$T/argv"
    export PATH STUB_ARGV STUB_VERSION_OUT STUB_VERSION_RC STUB_VERSION_ERR STUB_ENV_OUT STUB_ENV_RC \
      STUB_WALK_OUT STUB_WALK_RC STUB_EXEC_SLEEP STUB_EXEC_RC
    # shellcheck disable=SC2034 # ROOT is read by the function under test.
    ROOT="$FX_ROOT"
    set -uo pipefail
    "$@"
  ) >"$o" 2>"$e" || rc=$?
  if [ "$want_argv" = "none" ]; then
    if [ -e "$T/argv" ]; then
      say_fail "$row" "the stub docker was called, and it must not be" "$T/argv" "$o" "$e"
      return 0
    fi
  elif ! cmp -s "$want_argv" "$T/argv"; then
    diff "$want_argv" "$T/argv" > "$T/$row.argv-diff" 2>&1 || true
    say_fail "$row" "the stub docker got a different argv (< expected, > got)" "$T/$row.argv-diff" "$o" "$e"
    return 0
  fi
  check_row "$row" "$rc" "$want_rc" "$present" "$absent" "$o" "$e"
}

stub_reset
run_fn N0 0 "" "::warning::" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"
stub_reset; STUB_VERSION_OUT="v24.14.0"
run_fn N1 0 "::warning::iam-console: the runtime image runs Node 24.14.0|refresh closes the gap" "::error::" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"
stub_reset; STUB_VERSION_OUT="v24.18.0"
run_fn N1b 0 "::warning::iam-console: the runtime image runs Node 24.18.0|bump .prototools" "::error::" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"
stub_reset; STUB_VERSION_OUT="v23.1.0"
run_fn N2 1 "a different major" "" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"
stub_reset; STUB_VERSION_OUT="garbage"
run_fn N3 1 "could not parse" "" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"
stub_reset; STUB_VERSION_OUT=""; STUB_VERSION_RC=125
run_fn N4 1 "NOT checked — docker exited 125" "" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"
stub_reset; FX_ROOT="$T/fx-nopin"
run_fn N5 1 'no node = "X.Y.Z" pin' "" none console_node_version_row iam-console "$T_IMAGE"
# docker prints a platform-mismatch WARNING on stderr when an amd64 image runs on an arm64 host.
# Only stdout is parsed, so the row stays green.
stub_reset; STUB_VERSION_ERR="WARNING: The requested image's platform (linux/amd64) does not match the detected host platform (linux/arm64/v8)"
run_fn N6 0 "" "could not parse" "$T/argv-node" console_node_version_row iam-console "$T_IMAGE"

# --- call-site rows (P1, P2) -------------------------------------------------------------------
# A row function that smoke_consoles no longer calls proves nothing, and every row above stays
# green. So the call lines are pinned as WHOLE lines (leading blanks allowed), `|| ec=1` included.
# Each pin has a mutation that deletes its line, and the pin must then red.
count_line() {
  awk -v want="$2" '{ l = $0; sub(/^[[:space:]]+/, "", l); if (l == want) n++ } END { print n + 0 }' "$1"
}

drop_line() {
  awk -v want="$2" '{ l = $0; sub(/^[[:space:]]+/, "", l); if (l != want) print }' "$1"
}

# pin_rows <row> <fn-file> <line> — the pin on the real text, then on a copy without the line.
pin_rows() {
  local row="$1" file="$2" line="$3" n
  n="$(count_line "$file" "$line")"
  if [ "$n" -eq 1 ]; then
    say_pass "$row"
  else
    say_fail "$row" "found ${n} copies of the line '${line}' in ${file##*/}, expected exactly 1"
  fi
  drop_line "$file" "$line" > "$T/$row-mut.sh"
  if cmp -s "$file" "$T/$row-mut.sh"; then
    say_fail "$row-mut" "mutation did not apply"
    return 0
  fi
  n="$(count_line "$T/$row-mut.sh" "$line")"
  if [ "$n" -eq 0 ]; then
    say_pass "$row-mut"
  else
    say_fail "$row-mut" "the pin still found the line after the mutation deleted it"
  fi
}

# shellcheck disable=SC2016 # the pinned call line is literal text
pin_rows P1a "$T/fn-smoke_consoles.sh" 'console_node_version_row "$app" "$image" || ec=1'

# --- console_image_config_row (E rows) ---------------------------------------------------------
# The argv that console_image_config_row must hand to docker: the Config.Env read, then the walk.
printf '%s\n' image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$T_IMAGE" -- \
  run --rm --entrypoint /nodejs/bin/node "$T_IMAGE" -e '<multi-line>' /app -- > "$T/argv-config"

stub_reset
run_fn E0 0 "" "::error::" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
# The error names the key and never the value.
stub_reset; STUB_ENV_OUT="$(printf '%s\n' 'PATH=/usr/bin' 'PAIGASUS_X=secret-value')"
run_fn E1 1 "bakes PAIGASUS_X" "secret-value" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
stub_reset; STUB_WALK_OUT="$(printf '%s\n' 'walked=1300' '/app/apps/x/.env')"
run_fn E2 1 "holds .env file" "" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
stub_reset; STUB_WALK_OUT="$(printf '%s\n' 'walked=1300' '/app/node_modules/p/.env.example')"
run_fn E2b 0 "" "::error::" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
stub_reset; STUB_ENV_OUT=""; STUB_ENV_RC=1
run_fn E3 1 "image config NOT checked" "" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
stub_reset; STUB_WALK_OUT=""; STUB_WALK_RC=125
run_fn E4 1 ".env scan NOT checked" "" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
stub_reset; STUB_WALK_OUT="walked=0"
run_fn E5 1 "too few to prove anything" "" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"
# E6: a grep rc > 1 (grep itself could not run) must not silently clear the .env scan. A stub
# `grep` ahead of the docker stub on PATH always exits 2; console_image_config_row's only grep
# call is the node_modules filter, so this is an honest stand-in for "grep could not run" without
# touching the harness's own grep-free control flow (check_row/say_fail use `case`, not grep).
mkdir -p "$T/stub-grep"
printf '%s\n' '#!/usr/bin/env bash' 'exit 2' > "$T/stub-grep/grep"
chmod +x "$T/stub-grep/grep"
stub_reset; STUB_PATH_EXTRA="$T/stub-grep"
run_fn E6 1 ".env scan NOT checked — grep exited 2" "" "$T/argv-config" console_image_config_row iam-console "$T_IMAGE"

# shellcheck disable=SC2016 # the pinned call line is literal text
pin_rows P1b "$T/fn-smoke_consoles.sh" 'console_image_config_row "$app" "$image" || ec=1'

# --- console_healthcheck_row (H rows) ----------------------------------------------------------
# The argv that console_healthcheck_row must hand to docker (through with_deadline).
printf '%s\n' exec smoke-selftest /nodejs/bin/node /app/healthcheck.mjs -- > "$T/argv-health"

stub_reset
run_fn H0 0 "" "::error::" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 5
stub_reset; STUB_EXEC_SLEEP=10
run_fn H1 1 "did not finish within 2s" "" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 2
stub_reset; STUB_EXEC_RC=1
run_fn H2 1 "exited 1" "did not finish" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 5
stub_reset; STUB_EXEC_RC=137
run_fn H3 1 "exited 137" "did not finish" "$T/argv-health" console_healthcheck_row smoke-selftest iam-console /iam 5
stub_reset
run_fn H4 1 "positive integer" "" none console_healthcheck_row smoke-selftest iam-console /iam abc
stub_reset
run_fn H5 1 "positive integer" "" none console_healthcheck_row smoke-selftest iam-console /iam 0
# H6 (SMA-670 review finding 2): a deadline of more than 5 digits must be refused by the `case`
# itself, before `[ "$deadline" -lt 1 ]` ever runs on it. That comparison overflows bash's integer
# test on a value this size, exits 2 (an error, not a verdict), and the `||` above it then read
# that 2 as "false" — so this huge deadline was ACCEPTED and docker was called, fail-open.
stub_reset
run_fn H6 1 "positive integer" "" none console_healthcheck_row smoke-selftest iam-console /iam 99999999999999999999

# shellcheck disable=SC2016 # the pinned call lines are literal text
pin_rows P1c "$T/fn-smoke_consoles.sh" 'console_healthcheck_row "$name" "$app" "$base_path" "$CONSOLE_HC_DEADLINE" || ec=1'
# shellcheck disable=SC2016 # the pinned call lines are literal text
pin_rows P2 "$T/fn-console_healthcheck_row.sh" 'with_deadline "$deadline" docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs >"$out" 2>&1 || hc_rc=$?'

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
