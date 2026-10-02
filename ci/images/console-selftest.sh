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
# dispatch code at the end exits before any function could run. A copied ROW function reads no
# global except ROOT, which run_fn sets to FX_ROOT inside each row's subshell. smoke_consoles reads
# more globals; smoke_case sets them and stubs every row that needs a real image.
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
# SMA-671: no row reads the parity mode from the environment. A developer's shell or a future
# job-level env must not change a row; the PF rows set the variable inside their own wrapper.
unset CONSOLE_PARITY_REQUIRED

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
RUN_SH="$HERE/run.sh"

# The functions copied out of run.sh. A task that adds a function to run.sh adds its name here.
FUNCS="assert_console_pins with_deadline console_node_version_row smoke_consoles console_image_config_row console_healthcheck_row console_smoke_cleanup app_for base_path_for console_probe_path_for console_new_sid console_container_args console_smoke_redis_start console_seed_session console_kernel_route_row console_kernel_control_row console_kernel_control_probe kernel_control_flag parity_required_flag console_staged_parity_row docker_context_leaks"

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
      # SMA-675: `docker run -d` starts the Redis sidecar.
      if [ "${2:-}" = "-d" ]; then exit "$STUB_RUND_RC"; fi
      for a in "$@"; do
        if [ "$a" = "--version" ]; then
          if [ -n "$STUB_VERSION_ERR" ]; then printf '%s\n' "$STUB_VERSION_ERR" >&2; fi
          if [ -n "$STUB_VERSION_OUT" ]; then printf '%s\n' "$STUB_VERSION_OUT"; fi
          exit "$STUB_VERSION_RC"
        fi
      done
      # SMA-675: the kernel control row's walk for the wasm chunks.
      for a in "$@"; do
        case "$a" in
          */.next/server/chunks)
            if [ -n "$STUB_CHUNKS_OUT" ]; then printf '%s\n' "$STUB_CHUNKS_OUT"; fi
            exit "$STUB_CHUNKS_RC"
            ;;
        esac
      done
      if [ -n "$STUB_WALK_OUT" ]; then printf '%s\n' "$STUB_WALK_OUT"; fi
      exit "$STUB_WALK_RC"
      ;;
    image)
      if [ -n "$STUB_ENV_OUT" ]; then printf '%s\n' "$STUB_ENV_OUT"; fi
      exit "$STUB_ENV_RC"
      ;;
    exec)
      # SMA-675: `docker exec <redis> redis-cli <command> …`.
      if [ "${3:-}" = "redis-cli" ]; then
        case "${4:-}" in
          PING) printf '%s\n' "$STUB_PING_OUT"; exit 0 ;;
          TIME)
            if [ -n "$STUB_TIME_OUT" ]; then printf '%s\n' "$STUB_TIME_OUT"; fi
            exit "$STUB_TIME_RC"
            ;;
          SET) printf '%s\n' "$STUB_SET_OUT"; exit "$STUB_SET_RC" ;;
        esac
      fi
      if [ "$STUB_EXEC_SLEEP" -gt 0 ]; then exec sleep "$STUB_EXEC_SLEEP"; fi
      exit "$STUB_EXEC_RC"
      ;;
    network) exit "$STUB_NET_RC" ;;
    create)
      # SMA-675 D4: X11 records the cleanup registry that the caller exported at create time.
      if [ -n "${STUB_NAMES_FILE:-}" ]; then
        printf '%s\n' "${CONSOLE_SMOKE_NAMES-<unset>}" > "$STUB_NAMES_FILE"
      fi
      if [ "$STUB_CREATE_RC" -eq 0 ]; then echo "stub-container-id"; fi
      exit "$STUB_CREATE_RC"
      ;;
    start) exit "$STUB_START_RC" ;;
    cp) exit "$STUB_CP_RC" ;;
    port) printf '%s\n' "$STUB_PORT_OUT"; exit 0 ;;
    logs)
      if [ -n "$STUB_LOGS_OUT" ]; then printf '%s\n' "$STUB_LOGS_OUT"; fi
      exit 0
      ;;
    rm) exit "$STUB_RM_RC" ;;
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

# SMA-675: the stub curl. Call <n> (counted in $STUB_CURL_DIR/count) answers from
# $STUB_CURL_DIR/<n>.w (printed as the -w output), <n>.body (written to the -o file) and <n>.rc
# (its exit code). A call with no files of its own answers like the last call that has them. It
# records its argv in $STUB_CURL_DIR/argv in the stub docker's format.
stub_curl_main() {
  local n=0 a o="" prev="" k
  if [ -s "$STUB_CURL_DIR/count" ]; then n="$(cat "$STUB_CURL_DIR/count")"; fi
  n=$((n + 1))
  printf '%s\n' "$n" > "$STUB_CURL_DIR/count"
  for a in "$@"; do
    if [ "$prev" = "-o" ]; then o="$a"; fi
    prev="$a"
    printf '%s\n' "$a" >> "$STUB_CURL_DIR/argv"
  done
  printf '%s\n' "--" >> "$STUB_CURL_DIR/argv"
  k="$n"
  while [ "$k" -gt 1 ] && [ ! -e "$STUB_CURL_DIR/$k.w" ]; do k=$((k - 1)); done
  if [ -n "$o" ] && [ "$o" != "/dev/null" ]; then
    : > "$o"
    if [ -e "$STUB_CURL_DIR/$k.body" ]; then cat "$STUB_CURL_DIR/$k.body" > "$o"; fi
  fi
  if [ -e "$STUB_CURL_DIR/$k.w" ]; then printf '%s' "$(cat "$STUB_CURL_DIR/$k.w")"; fi
  if [ -e "$STUB_CURL_DIR/$k.rc" ]; then exit "$(cat "$STUB_CURL_DIR/$k.rc")"; fi
  exit 0
}
{
  printf '%s\n' '#!/usr/bin/env bash'
  declare -f stub_curl_main
  # shellcheck disable=SC2016 # the line is written into the stub file literally
  printf '%s\n' 'stub_curl_main "$@"'
} > "$T/stub/curl"
chmod +x "$T/stub/curl"

# curl_reset <name> — a new, empty answer directory for the stub curl.
curl_reset() {
  STUB_CURL_DIR="$T/curl-$1"
  rm -rf "$STUB_CURL_DIR"
  mkdir -p "$STUB_CURL_DIR"
}

# curl_resp <n> <w-output> <body> <rc> — the answer of stub curl call <n>.
curl_resp() {
  printf '%s' "$2" > "$STUB_CURL_DIR/$1.w"
  printf '%s' "$3" > "$STUB_CURL_DIR/$1.body"
  printf '%s\n' "$4" > "$STUB_CURL_DIR/$1.rc"
}

# curl_count_is <row> <n> — a PASS row <row>-count when the stub curl was called exactly <n> times.
curl_count_is() {
  local got=0
  if [ -s "$STUB_CURL_DIR/count" ]; then got="$(cat "$STUB_CURL_DIR/count")"; fi
  if [ "$got" -eq "$2" ]; then
    say_pass "$1-count"
  else
    say_fail "$1-count" "the stub curl was called ${got} times, expected $2" "$STUB_CURL_DIR/argv"
  fi
}

# argv_calls <argv-file> — one line per stub call, its arguments joined by one space.
argv_calls() {
  awk '$0 == "--" { print line; line = ""; next } { line = (line == "" ? $0 : line " " $0) }' "$1"
}

# expect_call <row> <substring> — a PASS row <row>-argv when one stub docker call of the last
# run_fn holds <substring>. expect_no_call is the reverse.
expect_call() {
  local calls=""
  if [ -e "$T/argv" ]; then calls="$(argv_calls "$T/argv")"; fi
  case "$calls" in
    *"$2"*) say_pass "$1-argv" ;;
    *) say_fail "$1-argv" "no stub docker call holds '$2'" "$T/argv" ;;
  esac
}
expect_no_call() {
  local calls=""
  if [ -e "$T/argv" ]; then calls="$(argv_calls "$T/argv")"; fi
  case "$calls" in
    *"$2"*) say_fail "$1-argv" "a stub docker call holds '$2', and none may" "$T/argv" ;;
    *) say_pass "$1-argv" ;;
  esac
}

# count_calls <prefix> — the number of stub docker calls of the last run_fn that start with
# <prefix>.
count_calls() {
  if [ ! -e "$T/argv" ]; then echo 0; return 0; fi
  argv_calls "$T/argv" | awk -v p="$1" 'index($0, p) == 1 { n++ } END { print n + 0 }'
}

# expect_in <row> <file> <substring> / expect_not_in — a PASS row when <file> holds (or does not
# hold) <substring>. `case`, not grep: no regex and no early-exit reader.
expect_in() {
  local text=""
  if [ -e "$2" ]; then text="$(cat "$2")"; fi
  case "$text" in
    *"$3"*) say_pass "$1" ;;
    *) say_fail "$1" "${2##*/} does not hold '$3'" "$2" ;;
  esac
}
expect_not_in() {
  local text=""
  if [ -e "$2" ]; then text="$(cat "$2")"; fi
  case "$text" in
    *"$3"*) say_fail "$1" "${2##*/} holds '$3', which it must not" "$2" ;;
    *) say_pass "$1" ;;
  esac
}

stub_reset() {
  STUB_VERSION_OUT="v24.16.0"; STUB_VERSION_RC=0; STUB_VERSION_ERR=""
  STUB_ENV_OUT="PATH=/usr/bin"; STUB_ENV_RC=0
  STUB_WALK_OUT="walked=1300"; STUB_WALK_RC=0
  STUB_EXEC_SLEEP=0; STUB_EXEC_RC=0
  # SMA-675 defaults: every new docker command succeeds.
  STUB_RUND_RC=0
  STUB_CHUNKS_OUT="/app/apps/iam-console/.next/server/chunks/ssr/x_paigasus_wasm_bg_1.wasm"; STUB_CHUNKS_RC=0
  STUB_PING_OUT="PONG"
  STUB_TIME_OUT="$(printf '%s\n' 1790000000 123456)"; STUB_TIME_RC=0
  STUB_SET_OUT="OK"; STUB_SET_RC=0
  STUB_NET_RC=0; STUB_CREATE_RC=0; STUB_START_RC=0; STUB_CP_RC=0; STUB_RM_RC=0
  STUB_PORT_OUT="0.0.0.0:32768"
  STUB_LOGS_OUT="CompileError: WebAssembly.Module(): expected magic word 00 61 73 6d"
  curl_reset default
  FX_ROOT="$T/fx-node"
  # A row-scoped extra PATH entry, prepended ahead of the docker stub, empty by default. E6 and K7
  # (the grep-rc-2 rows) set it.
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

# run_fn <row> <want_rc> <present> <absent> <want-argv-file|none|any> <fn> [<arg>...] — the
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
      STUB_WALK_OUT STUB_WALK_RC STUB_EXEC_SLEEP STUB_EXEC_RC STUB_RUND_RC STUB_CHUNKS_OUT \
      STUB_CHUNKS_RC STUB_PING_OUT STUB_TIME_OUT STUB_TIME_RC STUB_SET_OUT STUB_SET_RC STUB_NET_RC \
      STUB_CREATE_RC STUB_START_RC STUB_CP_RC STUB_RM_RC STUB_PORT_OUT STUB_LOGS_OUT STUB_CURL_DIR
    # shellcheck disable=SC2034 # ROOT is read by the function under test.
    ROOT="$FX_ROOT"
    set -uo pipefail
    "$@"
  ) >"$o" 2>"$e" || rc=$?
  if [ "$want_argv" = "any" ]; then
    :
  elif [ "$want_argv" = "none" ]; then
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

# --- SMA-671: console_staged_parity_row (SP rows) --------------------------------------------
# The argv that the row must hand to docker: the staged-tree walk of the IMAGE, never of a name
# that the row builds from <app> (the SMA-688 rule).
printf '%s\n' run --rm --entrypoint /nodejs/bin/node "$T_IMAGE" -e '<multi-line>' /app/apps/iam-console -- > "$T/argv-parity"

# sp_fx <name> <build-id> <file>... — a fake host build of iam-console under $T/fx-sp-<name>, and
# FX_ROOT points at it (call it AFTER stub_reset, which resets FX_ROOT). .next/BUILD_ID holds
# <build-id>; there is no BUILD_ID file when <build-id> is empty. Each <file> is made under
# .next/static, and a leading <BUILD_ID>/ becomes <build-id>/.
sp_fx() {
  local name="$1" id="$2" std f
  shift 2
  FX_ROOT="$T/fx-sp-$name"
  std="$FX_ROOT/ts/apps/iam-console/.next/standalone/apps/iam-console"
  rm -rf "$FX_ROOT"
  mkdir -p "$std/.next/static"
  if [ -n "$id" ]; then printf '%s\n' "$id" > "$std/.next/BUILD_ID"; fi
  for f in "$@"; do
    case "$f" in "<BUILD_ID>/"*) f="$id/${f#<BUILD_ID>/}" ;; esac
    mkdir -p "$std/.next/static/$(dirname "$f")"
    : > "$std/.next/static/$f"
  done
}
SP_ID="spBuildId0123"
# What the image walk prints for a tree equal to `sp_fx … chunks/a.js '<BUILD_ID>/_buildManifest.js'`.
SP_WALK_OK="$(printf '%s\n' public=0 'chunks/a.js' '<BUILD_ID>/_buildManifest.js')"

stub_reset; FX_ROOT="$T/fx-sp-none"; mkdir -p "$FX_ROOT"
run_fn SP1 0 "" "::error::" none console_staged_parity_row iam-console "$T_IMAGE" optional
expect_in SP1-out "$T/SP1.out" "staged-tree parity NOT CHECKED"
stub_reset; FX_ROOT="$T/fx-sp-none"
run_fn SP2 1 "parity was required but NOT checked" "" none console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; FX_ROOT="$T/fx-sp-none"
run_fn SP2b 1 "the third argument must be required or optional, not 'maybe'" "" none console_staged_parity_row iam-console "$T_IMAGE" maybe
stub_reset; sp_fx ok "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP3 0 "" "::error::" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
expect_in SP3-out "$T/SP3.out" "staged tree matches the host build (2 files, public=0)"
stub_reset; sp_fx ok "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP3b 0 "" "::error::" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" optional
expect_in SP3b-out "$T/SP3b.out" "staged tree matches the host build (2 files, public=0)"
stub_reset; sp_fx pub "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'
mkdir -p "$FX_ROOT/ts/apps/iam-console/.next/standalone/apps/iam-console/public"
STUB_WALK_OUT="$(printf '%s\n' public=1 'chunks/a.js' '<BUILD_ID>/_buildManifest.js')"
run_fn SP3c 0 "" "::error::" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
expect_in SP3c-out "$T/SP3c.out" "staged tree matches the host build (2 files, public=1)"
stub_reset; sp_fx dirs "$SP_ID" media/f.woff '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP4 1 "different top-level directories" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx files "$SP_ID" chunks/b.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP4b 1 "the chunk-name assumption failing" "Re-run 'moon run" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx files "$SP_ID" chunks/b.js '<BUILD_ID>/_buildManifest.js'; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP4c 1 "DIFFERENT sources" "the chunk-name assumption failing" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" optional
stub_reset; sp_fx ok "$SP_ID" chunks/a.js '<BUILD_ID>/_buildManifest.js'
STUB_WALK_OUT="$(printf '%s\n' public=1 'chunks/a.js' '<BUILD_ID>/_buildManifest.js')"
run_fn SP4d 1 "disagree on staging public/" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx ok "$SP_ID" chunks/a.js; STUB_WALK_OUT="Error: ENOENT: no such file or directory, scandir '/app/apps/iam-console/.next/static'"; STUB_WALK_RC=1
run_fn SP5 1 "the staging copy in ts/Dockerfile did not run" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx ok "$SP_ID" chunks/a.js; STUB_WALK_OUT=""; STUB_WALK_RC=125
run_fn SP5b 1 "NOT checked — docker exited 125" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
# Review Focus 5: an empty walk and a missing BUILD_ID are named errors, never a pass.
stub_reset; sp_fx ok "$SP_ID" chunks/a.js; STUB_WALK_OUT=""; STUB_WALK_RC=0
run_fn SP5c 1 "printed nothing" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required
stub_reset; sp_fx noid "" chunks/a.js; STUB_WALK_OUT="$SP_WALK_OK"
run_fn SP5d 1 "has no .next/BUILD_ID" "" "$T/argv-parity" console_staged_parity_row iam-console "$T_IMAGE" required

# shellcheck disable=SC2016 # the pinned call line is literal text
pin_rows P1d "$T/fn-smoke_consoles.sh" 'console_staged_parity_row "$app" "$image" "$parity" || ec=1'

# --- SMA-671: docker_context_leaks (CX rows) ---------------------------------------------------
# A fixture git repository with one commit. It commits, so it follows ci/CLAUDE.md (SMA-708,
# SMA-714): no maintenance, no gc, no signing, no global or system config.
cx_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$FX_ROOT" "$@"; }
# cx_fx <name> [<dockerignore-file>] — the fixture, with ts/.dockerignore copied from the real one
# (or from <dockerignore-file>), a root .gitignore like the repo's, two tracked files under
# ts/apps/a and one under rs/crates/bindings/b. Call it AFTER stub_reset; FX_ROOT points at it.
cx_fx() {
  FX_ROOT="$T/fx-cx-$1"
  rm -rf "$FX_ROOT"
  mkdir -p "$FX_ROOT/ts/apps/a" "$FX_ROOT/rs/crates/bindings/b"
  cp "${2:-$REPO/ts/.dockerignore}" "$FX_ROOT/ts/.dockerignore"
  printf '%s\n' 'node_modules/' '.next/' '*.tsbuildinfo' > "$FX_ROOT/.gitignore"
  printf '%s\n' 'export {};' > "$FX_ROOT/ts/apps/a/page.ts"
  printf '%s\n' '/// <reference types="next" />' > "$FX_ROOT/ts/apps/a/next-env.d.ts"
  printf '%s\n' 'module.exports = {};' > "$FX_ROOT/rs/crates/bindings/b/index.js"
  cx_git init -q || infra "git init failed for the CX fixture"
  cx_git config maintenance.auto false
  cx_git config gc.auto 0
  cx_git config commit.gpgsign false
  cx_git config tag.gpgsign false
  cx_git add -A || infra "git add failed for the CX fixture"
  cx_git -c user.name=selftest -c user.email=selftest@example.invalid commit -q -m fixture \
    || infra "the CX fixture commit failed"
}
# What a host build leaves that ts/.dockerignore DOES exclude.
cx_excluded() {
  mkdir -p "$FX_ROOT/ts/node_modules/p" "$FX_ROOT/ts/apps/a/node_modules/q" "$FX_ROOT/ts/apps/a/.next/static"
  : > "$FX_ROOT/ts/node_modules/p/index.js"
  : > "$FX_ROOT/ts/apps/a/node_modules/q/index.js"
  : > "$FX_ROOT/ts/apps/a/.next/static/x.js"
  : > "$FX_ROOT/ts/apps/a/.env.local"
}

stub_reset; cx_fx 0; cx_excluded
run_fn CX0 0 "" "::error::" none docker_context_leaks
expect_in CX0-out "$T/CX0.out" "docker context: no host artifact"
stub_reset; cx_fx 1; cx_excluded; : > "$FX_ROOT/ts/apps/a/tsconfig.tsbuildinfo"
run_fn CX1 1 "docker context: 'ts/apps/a/tsconfig.tsbuildinfo' (git status '!!')" "node_modules" none docker_context_leaks
stub_reset; cx_fx 2; : > "$FX_ROOT/ts/apps/a/new.txt"
run_fn CX2 1 "docker context: 'ts/apps/a/new.txt' (git status '??')" "" none docker_context_leaks
# Review Focus 3: a tracked file that the build rewrites is a leak too.
stub_reset; cx_fx 3; printf '%s\n' '// changed' >> "$FX_ROOT/ts/apps/a/next-env.d.ts"
run_fn CX3 1 "docker context: 'ts/apps/a/next-env.d.ts' (git status ' M')" "" none docker_context_leaks
stub_reset; cx_fx 4; : > "$FX_ROOT/rs/crates/bindings/b/x.node"
run_fn CX4 1 "docker context: 'rs/crates/bindings/b/x.node'" "" none docker_context_leaks
printf '%s\n' '**/node_modules' '!keep' > "$T/cx5-ignore"
stub_reset; cx_fx 5 "$T/cx5-ignore"
run_fn CX5 1 "cannot read the pattern '!keep'" "" none docker_context_leaks
printf '%s\n' '**/node_modules' 'apps/a/x' > "$T/cx5b-ignore"
stub_reset; cx_fx 5b "$T/cx5b-ignore"
run_fn CX5b 1 "cannot read the pattern 'apps/a/x'" "" none docker_context_leaks
# A pattern without **/ excludes the top level only, as in Docker.
printf '%s\n' 'node_modules' '**/.next' > "$T/cx6-ignore"
stub_reset; cx_fx 6 "$T/cx6-ignore"; cx_excluded
run_fn CX6 1 "docker context: 'ts/apps/a/node_modules/'" "'ts/node_modules/'" none docker_context_leaks
# Review Focus 3: a path with a space is named whole (-z output, no quoting).
stub_reset; cx_fx 7; : > "$FX_ROOT/ts/apps/a/my file.txt"
run_fn CX7 1 "docker context: 'ts/apps/a/my file.txt'" "" none docker_context_leaks
cx_nogit() { export GIT_CEILING_DIRECTORIES="$T"; docker_context_leaks; }
stub_reset; FX_ROOT="$T/fx-cx-nogit"; mkdir -p "$FX_ROOT/ts"; cp "$REPO/ts/.dockerignore" "$FX_ROOT/ts/.dockerignore"
run_fn CX8 1 "docker context NOT checked — git status exited" "" none cx_nogit

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

# --- SMA-675: console_new_sid and console_container_args (SID, A rows) ------------------------
stub_reset
run_fn SID0 0 "" "::error::" none console_new_sid
SID0_OUT="$(cat "$T/SID0.out")"
case "$SID0_OUT" in
  *[!0-9a-f]*|'') say_fail SID0-shape "the sid is not lowercase hex: '${SID0_OUT}'" ;;
  *) if [ "${#SID0_OUT}" -eq 64 ]; then say_pass SID0-shape; else say_fail SID0-shape "the sid has ${#SID0_OUT} characters, expected 64"; fi ;;
esac

printf '%s\n' -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com" > "$T/a-env"
A_ZONES='{"iam":"/iam","gateway":"/gateway"}'
printf '%s\n' -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com" -e "PAIGASUS_ZONE=iam" \
  -e "PAIGASUS_ZONES=${A_ZONES}" --network smoke-net-1 -e "PAIGASUS_SESSION_STORE=redis" \
  -e "PAIGASUS_SESSION_REDIS_URL=redis://smoke-redis-1:6379" --add-host iam:127.0.0.1 \
  --add-host gateway:127.0.0.1 -p 0:3000 > "$T/a-want-net"
printf '%s\n' -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com" -e "PAIGASUS_ZONE=iam" \
  -e "PAIGASUS_ZONES=${A_ZONES}" -e "PAIGASUS_SESSION_STORE=memory" -p 0:3000 > "$T/a-want-mem"

# a_cmp <row> <want-file> — a PASS row <row>-list when the row's stdout is exactly <want-file>.
a_cmp() {
  if cmp -s "$2" "$T/$1.out"; then
    say_pass "$1-list"
  else
    diff "$2" "$T/$1.out" > "$T/$1.list-diff" 2>&1 || true
    say_fail "$1-list" "a different argument list (< expected, > got)" "$T/$1.list-diff"
  fi
}

stub_reset
run_fn A0 0 "" "::error::" none console_container_args iam-console iam "$A_ZONES" smoke-net-1 "redis://smoke-redis-1:6379" "$T/a-env"
a_cmp A0 "$T/a-want-net"
stub_reset
run_fn A1 0 "" "::error::" none console_container_args iam-console iam "$A_ZONES" "" "" "$T/a-env"
a_cmp A1 "$T/a-want-mem"
stub_reset
run_fn A2 1 "with no Redis URL" "" none console_container_args iam-console iam "$A_ZONES" smoke-net-1 "" "$T/a-env"
stub_reset
run_fn A3 1 "is not readable" "" none console_container_args iam-console iam "$A_ZONES" "" "" "$T/a-env-missing"

# --- SMA-675: Redis sidecar, seeded session and cleanup (R, S, C rows) ------------------------
stub_reset
run_fn R0 0 "" "::error::" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
expect_call R0 "network create --label paigasus.smoke=console smoke-net-1"
expect_call R0 "run -d --name smoke-redis-1 --network smoke-net-1 --label paigasus.smoke=console redis:stub"
stub_reset; STUB_NET_RC=1
run_fn R1 1 "the per-run network smoke-net-1 was not created" "" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
expect_no_call R1 "run -d"
stub_reset; STUB_RUND_RC=125
run_fn R2 1 "the Redis sidecar smoke-redis-1 did not start" "" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
stub_reset; STUB_PING_OUT="LOADING"
run_fn R3 1 "did not answer PONG within 2 tries" "" any console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub 2
stub_reset
run_fn R4 1 "is not a positive integer" "" none console_smoke_redis_start smoke-net-1 smoke-redis-1 redis:stub x

# The seeded record. TIME 1790000000 s + 123456 us = 1790000000123 ms; + 600000 = 1790000600123.
S_SID="$(printf '0123456789abcdef%.0s' 1 2 3 4)"
stub_reset
run_fn S0 0 "" "::error::" any console_seed_session smoke-redis-1 "$S_SID"
expect_call S0 "exec smoke-redis-1 redis-cli SET pgs:sess:${S_SID} "
expect_call S0 '"email":"smoke-0123456789ab@example.com"'
expect_call S0 '"version":2,"rev":1,'
expect_call S0 '"accessExpiresAt":1790000600123,"absoluteExpiresAt":1790000600123,'
expect_call S0 '"principalPrn":null,'
expect_call S0 " PX 600000"
expect_no_call S0 "refreshToken"
stub_reset; STUB_SET_OUT="ERR wrong number of arguments"
run_fn S1 1 "replied 'ERR wrong number of arguments', not OK" "" any console_seed_session smoke-redis-1 "$S_SID"
stub_reset; STUB_TIME_OUT=""; STUB_TIME_RC=1
run_fn S2 1 "'redis-cli TIME' on smoke-redis-1 exited 1" "" any console_seed_session smoke-redis-1 "$S_SID"
expect_no_call S2 " SET "
# Review Focus 2: a microsecond value with a leading zero is decimal, not octal.
# 012345 us = 12 ms, so 1790000000012 + 600000 = 1790000600012.
stub_reset; STUB_TIME_OUT="$(printf '%s\n' 1790000000 012345)"
run_fn S3 0 "" "::error::" any console_seed_session smoke-redis-1 "$S_SID"
expect_call S3 '"accessExpiresAt":1790000600012,'
stub_reset; STUB_TIME_OUT="$(printf '%s\n' 1790000000 089123)"
run_fn S4 0 "" "::error::" any console_seed_session smoke-redis-1 "$S_SID"
expect_call S4 '"accessExpiresAt":1790000600089,'

# cleanup_case <names> <network> — sets the two cleanup globals, then runs the cleanup.
cleanup_case() {
  # shellcheck disable=SC2034 # console_smoke_cleanup reads the two globals.
  CONSOLE_SMOKE_NAMES="$1"
  # shellcheck disable=SC2034 # console_smoke_cleanup reads the two globals.
  CONSOLE_SMOKE_NETWORK="$2"
  console_smoke_cleanup
}
printf '%s\n' rm -f smoke-a -- rm -f smoke-b -- network rm smoke-net-1 -- > "$T/argv-cleanup"
stub_reset
run_fn C0 0 "" "" "$T/argv-cleanup" cleanup_case " smoke-a smoke-b" smoke-net-1
printf '%s\n' rm -f smoke-a -- > "$T/argv-cleanup-nonet"
stub_reset
run_fn C1 0 "" "" "$T/argv-cleanup-nonet" cleanup_case " smoke-a" ""

# --- SMA-675: console_kernel_route_row (K rows) -----------------------------------------------
K_SID="$(printf '0123456789abcdef%.0s' 1 2 3 4)"
K_NONCE="smoke-0123456789ab@example.com"
K_BODY_OK="<html><title>Paigasus IAM</title><script>self.__next_f.push([1,\"{\\\"email\\\":\\\"${K_NONCE}\\\"}\"])</script></html>"
K_BODY_BRAND="<html><title>Paigasus IAM</title></html>"
K_PROXY="302 http://127.0.0.1:32768/iam/auth/login?returnTo=%2Fiam%2Forgs"
K_SESSION="307 http://127.0.0.1:32768/iam/auth/login?returnTo=%2Fiam%2F"
# k_row <row> <want_rc> <present> <absent> [<tries>] — the route row against the stub curl.
k_row() {
  run_fn "$1" "$2" "$3" "$4" any console_kernel_route_row http://127.0.0.1:32768 /iam /orgs "$K_SID" smoke-iam-console-1 "${5:-3}"
}

stub_reset; curl_reset K0; curl_resp 1 "200 " "$K_BODY_OK" 0
k_row K0 0 "" "::error::"
curl_count_is K0 1
expect_in K0-cookie "$STUB_CURL_DIR/argv" "Cookie: __Host-pgs_sid=${K_SID}"
expect_not_in K0-nofollow "$STUB_CURL_DIR/argv" "-L"
# Review Focus 1: the proxy value is tested first and is never retried.
stub_reset; curl_reset K1; curl_resp 1 "$K_PROXY" "" 0
k_row K1 1 "the proxy did not see the session cookie" "requireSession"
curl_count_is K1 1
stub_reset; curl_reset K2; curl_resp 1 "$K_SESSION" "" 0
k_row K2 1 "requireSession found no session after 3 requests" "the proxy did not see"
curl_count_is K2 3
stub_reset; curl_reset K3; curl_resp 1 "302 http://elsewhere.example/x" "" 0
k_row K3 1 "redirected (302) to 'http://elsewhere.example/x'" ""
curl_count_is K3 1
stub_reset; curl_reset K4; curl_resp 1 "404 " "" 0
k_row K4 1 "answered 404" ""
stub_reset; curl_reset K5; curl_resp 1 "500 " "" 0
k_row K5 1 "answered 500" ""
stub_reset; curl_reset K6; curl_resp 1 "000 " "" 7
k_row K6 1 "curl exited 7" ""
stub_reset; curl_reset K7; curl_resp 1 "200 " "$K_BODY_OK" 0; STUB_PATH_EXTRA="$T/stub-grep"
k_row K7 1 "grep exited 2" "holds no"
stub_reset; curl_reset K8; curl_resp 1 "200 " "$K_BODY_BRAND" 0
k_row K8 1 "the body holds no ${K_NONCE}" ""
stub_reset; curl_reset K9; curl_resp 1 "$K_SESSION" "" 0; curl_resp 2 "200 " "$K_BODY_OK" 0
k_row K9 0 "" "::error::"
curl_count_is K9 2
stub_reset; curl_reset K10
k_row K10 1 "is not a positive integer" "" abc
curl_count_is K10 0

# --- SMA-675: console_kernel_control_row (X rows) ---------------------------------------------
# Every X row asserts that the row removes its control container before it returns (SMA-675 Q2).
X_CTL="smoke-iam-console-nokernel-$$"
X_LINE="CompileError"
printf '%s\n' -e "PAIGASUS_ZONE=iam" --network smoke-net-1 -p 0:3000 > "$T/x-args"
X_CHUNK_A="/app/apps/iam-console/.next/server/chunks/ssr/a_paigasus_wasm_bg_1.wasm"
X_CHUNK_B="/app/apps/iam-console/.next/server/chunks/ssr/b_paigasus_wasm_bg_2.wasm"
# x_row <row> <want_rc> <present> <absent> [<kernel-line>] — the control row against the stubs.
x_row() {
  run_fn "$1" "$2" "$3" "$4" any console_kernel_control_row iam-console paigasus-iam-console:dev /iam /orgs smoke-redis-1 "$T/x-args" "${5-$X_LINE}"
  expect_call "$1-rm" "rm -f ${X_CTL}"
}

stub_reset; curl_reset X0; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X0 0 "" "::error::"
expect_call X0-create "create --name ${X_CTL} -e PAIGASUS_ZONE=iam --network smoke-net-1 -p 0:3000 paigasus-iam-console:dev"
expect_call X0-cp "cp "
expect_call X0-cp-target "${X_CTL}:${STUB_CHUNKS_OUT}"
expect_call X0-seed "exec smoke-redis-1 redis-cli SET pgs:sess:"
expect_in X0-healthz "$STUB_CURL_DIR/argv" "http://127.0.0.1:32768/iam/healthz"
expect_in X0-probe "$STUB_CURL_DIR/argv" "http://127.0.0.1:32768/iam/orgs"
stub_reset; STUB_CHUNKS_OUT=""; curl_reset X1
x_row X1 1 "found 0 *paigasus_wasm_bg*.wasm files" ""
expect_no_call X1 "create "
stub_reset; STUB_CHUNKS_OUT="$(printf '%s\n' "$X_CHUNK_A" "$X_CHUNK_B")"; curl_reset X2; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X2 0 "" "::error::"
X2_CP="$(count_calls "cp ")"
if [ "$X2_CP" -eq 2 ]; then say_pass X2-cp-count; else say_fail X2-cp-count "${X2_CP} docker cp calls, expected 2" "$T/argv"; fi
stub_reset; STUB_CP_RC=1; curl_reset X3
x_row X3 1 "docker cp exited 1" ""
expect_no_call X3 "start "
stub_reset; curl_reset X4; curl_resp 1 "200" "" 0; curl_resp 2 "200" "" 0
x_row X4 1 "answered 200 with every wasm chunk corrupted" ""
stub_reset; curl_reset X5; curl_resp 1 "200" "" 0; curl_resp 2 "302" "" 0
x_row X5 1 "answered '302'" ""
stub_reset; STUB_LOGS_OUT="AuthConfigError: PAIGASUS_SESSION_REDIS_URL is required"; curl_reset X6; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X6 1 "for a reason that is not the kernel" ""
stub_reset; curl_reset X7
x_row X7 1 "the kernel line is empty" "" ""
stub_reset; curl_reset X8; curl_resp 1 "503" "" 0
x_row X8 1 "never answered 200 on /iam/healthz" ""
stub_reset; STUB_CREATE_RC=125; curl_reset X9
x_row X9 1 "was not created from paigasus-iam-console:dev" ""
stub_reset; STUB_RM_RC=1; curl_reset X10; curl_resp 1 "200" "" 0; curl_resp 2 "500" "" 0
x_row X10 0 "::warning::iam-console: the kernel control container ${X_CTL} was not removed" "::error::"

# X11: spec D4 registers the control container in CONSOLE_SMOKE_NAMES BEFORE docker create, so
# the EXIT trap removes it after an abort. run_fn runs the row in a subshell, so the case exports
# the registry and the stub docker writes it to a file at `docker create`. docker create fails
# here (the abort path), and the name must still be registered, after the name already there.
x_reg_case() {
  CONSOLE_SMOKE_NAMES=" smoke-earlier"
  STUB_NAMES_FILE="$T/x11-names"
  export CONSOLE_SMOKE_NAMES STUB_NAMES_FILE
  console_kernel_control_row "$@"
}
stub_reset; STUB_CREATE_RC=125; curl_reset X11; rm -f "$T/x11-names"
run_fn X11 1 "was not created from paigasus-iam-console:dev" "" any x_reg_case iam-console paigasus-iam-console:dev /iam /orgs smoke-redis-1 "$T/x-args" "$X_LINE"
X11_NAMES="$(cat "$T/x11-names" 2>/dev/null || echo "<no docker create call>")"
if [ "$X11_NAMES" = " smoke-earlier ${X_CTL}" ]; then
  say_pass X11-registered
else
  say_fail X11-registered "CONSOLE_SMOKE_NAMES at docker create was '${X11_NAMES}', expected ' smoke-earlier ${X_CTL}'"
fi
# shellcheck disable=SC2016 # the pinned line is literal text
pin_rows X11-pin "$T/fn-console_kernel_control_row.sh" 'CONSOLE_SMOKE_NAMES="${CONSOLE_SMOKE_NAMES:-} ${ctl}"'

# --- SMA-675: the Q5 switch (KC, Z, D1 rows) --------------------------------------------------
# shellcheck disable=SC2034 # kernel_control_flag reads the variable.
kc_set() { PAIGASUS_SMOKE_KERNEL_CONTROL="$1"; kernel_control_flag; }
kc_unset() { unset PAIGASUS_SMOKE_KERNEL_CONTROL; kernel_control_flag; }
stub_reset
run_fn KC0 0 "" "::error::" none kc_unset
expect_in KC0-out "$T/KC0.out" "--kernel-control=on"
stub_reset
run_fn KC1 0 "" "::error::" none kc_set off
expect_in KC1-out "$T/KC1.out" "--kernel-control=off"
stub_reset
run_fn KC2 1 "PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off'" "" none kc_set bogus
# Review Focus 3: an empty value is a usage error, not a silent default.
stub_reset
run_fn KC3 1 "PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off'" "" none kc_set ""

# --- SMA-671: the parity switch (PF rows) -----------------------------------------------------
# shellcheck disable=SC2034 # parity_required_flag reads the variable.
pf_set() { CONSOLE_PARITY_REQUIRED="$1"; parity_required_flag; }
pf_unset() { unset CONSOLE_PARITY_REQUIRED; parity_required_flag; }
stub_reset
run_fn PF0 0 "" "::error::" none pf_unset
expect_in PF0-out "$T/PF0.out" "--parity=optional"
stub_reset
run_fn PF0b 0 "" "::error::" none pf_set ""
expect_in PF0b-out "$T/PF0b.out" "--parity=optional"
stub_reset
run_fn PF0c 0 "" "::error::" none pf_set 0
expect_in PF0c-out "$T/PF0c.out" "--parity=optional"
stub_reset
run_fn PF1 0 "" "::error::" none pf_set 1
expect_in PF1-out "$T/PF1.out" "--parity=required"
stub_reset
run_fn PF2 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1' (unset or empty means 0), not 'true'." "" none pf_set true
# Review Focus 1: a near miss is a usage error, never a silent optional.
stub_reset
run_fn PF3 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" none pf_set " 1"
expect_not_in PF3-out "$T/PF3.out" "--parity="
stub_reset
run_fn PF3b 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" none pf_set 01
expect_not_in PF3b-out "$T/PF3b.out" "--parity="
stub_reset
run_fn PF3c 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" none pf_set yes
expect_not_in PF3c-out "$T/PF3c.out" "--parity="

# smoke_case <args...> — smoke_consoles with its globals set to dummy values and every row that
# needs a real image replaced, so only the zone loop's wiring runs. Z_CALLS records which kernel
# rows smoke_consoles called. Z_REDIS_RC is what the Redis start returns.
Z_CALLS="$T/z-calls"
Z_REDIS_RC=0
# shellcheck disable=SC2034 # smoke_consoles reads the globals that the case sets.
smoke_case() {
  CONSOLE_SMOKE_ENV=(-e "PAIGASUS_OIDC_ISSUER=https://idp.example.com")
  CONSOLE_HC_DEADLINE=5
  RUN_ID="z"
  CONSOLE_SMOKE_NAMES=""
  CONSOLE_SMOKE_NETWORK=""
  CONSOLE_SMOKE_REDIS_IMAGE="redis:stub"
  CONSOLE_KERNEL_LINE="CompileError"
  assert_fresh() { return 0; }
  console_node_version_row() { return 0; }
  console_image_config_row() { return 0; }
  console_healthcheck_row() { return 0; }
  console_smoke_redis_start() { return "$Z_REDIS_RC"; }
  console_seed_session() { return 0; }
  console_kernel_route_row() { echo "route $2$3" >> "$Z_CALLS"; }
  console_kernel_control_row() { echo "control $1" >> "$Z_CALLS"; }
  console_staged_parity_row() { echo "parity $1 $3" >> "$Z_CALLS"; }
  smoke_consoles "$@"
}
# The stub curl answers the zone loop's three requests per zone in order: the page status (200),
# the cookie-less row (302), and the page body (no chunk URL, so the chunk rows red; Z rows do not
# read the rc).
z_curl() {
  curl_reset "$1"
  curl_resp 1 "200" "" 0; curl_resp 2 "302" "" 0; curl_resp 3 "<html></html>" "" 0
  curl_resp 4 "200" "" 0; curl_resp 5 "302" "" 0; curl_resp 6 "<html></html>" "" 0
}

stub_reset; z_curl Z1; rm -f "$Z_CALLS"; Z_REDIS_RC=0
run_fn Z1 1 "" "" any smoke_case --kernel-control=off --parity=optional iam=img:dev gateway=img:dev
expect_in Z1-skip-iam "$T/Z1.out" "iam-console: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off, the release path; SMA-675 Q5)"
expect_in Z1-skip-gw "$T/Z1.out" "gateway-console: kernel control row skipped (PAIGASUS_SMOKE_KERNEL_CONTROL=off"
expect_in Z1-route-iam "$Z_CALLS" "route /iam/orgs"
expect_in Z1-route-gw "$Z_CALLS" "route /gateway/overview"
expect_not_in Z1-nocontrol "$Z_CALLS" "control"
expect_no_call Z1 "-nokernel-"
expect_call Z1-net "--network smoke-net-z"
expect_call Z1-redis "PAIGASUS_SESSION_REDIS_URL=redis://smoke-redis-z:6379"
expect_no_call Z1-nomem "PAIGASUS_SESSION_STORE=memory"
expect_in Z1-parity-iam "$Z_CALLS" "parity iam-console optional"
expect_in Z1-parity-gw "$Z_CALLS" "parity gateway-console optional"
stub_reset; z_curl Z2; rm -f "$Z_CALLS"; Z_REDIS_RC=0
run_fn Z2 1 "" "" any smoke_case --kernel-control=on --parity=required iam=img:dev gateway=img:dev
expect_in Z2-control-iam "$Z_CALLS" "control iam-console"
expect_in Z2-control-gw "$Z_CALLS" "control gateway-console"
expect_not_in Z2-noskip "$T/Z2.out" "kernel control row skipped"
expect_in Z2-parity-iam "$Z_CALLS" "parity iam-console required"
expect_in Z2-parity-gw "$Z_CALLS" "parity gateway-console required"
stub_reset
run_fn Z3 1 "the first argument must be --kernel-control=on or --kernel-control=off, not 'iam=img:dev'" "" none smoke_case iam=img:dev
stub_reset
run_fn Z4 1 "not '--kernel-control=maybe'" "" none smoke_case --kernel-control=maybe iam=img:dev
# D6: with no Redis, the kernel rows do not run and the containers get the memory store.
stub_reset; z_curl Z5; rm -f "$Z_CALLS"; Z_REDIS_RC=1
run_fn Z5 1 "iam-console: kernel rows NOT run" "" any smoke_case --kernel-control=on --parity=optional iam=img:dev gateway=img:dev
expect_not_in Z5-norows "$Z_CALLS" "route"
expect_call Z5-mem "PAIGASUS_SESSION_STORE=memory"
expect_no_call Z5 "--network"
Z_REDIS_RC=0
# SMA-671: the second word is the parity mode. A bad or missing one stops before any docker call.
stub_reset
run_fn Z6 1 "the second argument must be --parity=required or --parity=optional, not '--parity=maybe'" "" none smoke_case --kernel-control=on --parity=maybe iam=img:dev
stub_reset
run_fn Z6b 1 "the second argument must be --parity=required or --parity=optional, not 'iam=img:dev'" "" none smoke_case --kernel-control=on iam=img:dev

# AC 3: CONSOLE_SMOKE_ENV sets no session store any more; console_container_args owns it.
awk '/^CONSOLE_SMOKE_ENV=\($/ { on = 1 } on { print } on && /^\)$/ { on = 0 }' "$RUN_SH" > "$T/env-block"
if [ -s "$T/env-block" ]; then say_pass ENV0-found; else say_fail ENV0-found "no CONSOLE_SMOKE_ENV=( block in run.sh"; fi
expect_not_in ENV0 "$T/env-block" "PAIGASUS_SESSION_STORE"

# D1: the dispatch arm, through the REAL script. It must stop before any docker call.
rm -f "$T/argv"
D1_RC=0
( PATH="$T/stub:$PATH"; STUB_ARGV="$T/argv"; export PATH STUB_ARGV
  PAIGASUS_SMOKE_KERNEL_CONTROL=bogus "$BASH" "$RUN_SH" all-consoles ) >"$T/D1.out" 2>"$T/D1.err" || D1_RC=$?
check_row D1 "$D1_RC" 1 "PAIGASUS_SMOKE_KERNEL_CONTROL must be 'on' or 'off'" "" "$T/D1.out" "$T/D1.err"
if [ -e "$T/argv" ]; then say_fail D1-nodocker "the stub docker was called" "$T/argv"; else say_pass D1-nodocker; fi
# SMA-671 D2 and D3: the two console dispatch arms, through the REAL script. A bad parity value
# stops each one before any docker call (the build of all-consoles included).
rm -f "$T/argv"
D2_RC=0
( PATH="$T/stub:$PATH"; STUB_ARGV="$T/argv"; export PATH STUB_ARGV
  CONSOLE_PARITY_REQUIRED=bogus "$BASH" "$RUN_SH" all-consoles ) >"$T/D2.out" 2>"$T/D2.err" || D2_RC=$?
check_row D2 "$D2_RC" 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" "$T/D2.out" "$T/D2.err"
if [ -e "$T/argv" ]; then say_fail D2-nodocker "the stub docker was called" "$T/argv"; else say_pass D2-nodocker; fi
rm -f "$T/argv"
D3_RC=0
( PATH="$T/stub:$PATH"; STUB_ARGV="$T/argv"; export PATH STUB_ARGV
  CONSOLE_PARITY_REQUIRED=bogus "$BASH" "$RUN_SH" smoke iam-console ) >"$T/D3.out" 2>"$T/D3.err" || D3_RC=$?
check_row D3 "$D3_RC" 1 "CONSOLE_PARITY_REQUIRED must be '0' or '1'" "" "$T/D3.out" "$T/D3.err"
if [ -e "$T/argv" ]; then say_fail D3-nodocker "the stub docker was called" "$T/argv"; else say_pass D3-nodocker; fi

# --- SMA-675 call-site pins (P3 to P16) --------------------------------------------------------
# shellcheck disable=SC2016 # the pinned lines are literal text
pin_rows P3 "$T/fn-smoke_consoles.sh" 'if console_smoke_redis_start "$CONSOLE_SMOKE_NETWORK" "$redis_name" "$CONSOLE_SMOKE_REDIS_IMAGE" 20; then kernel_ok=1; else ec=1; fi'
# shellcheck disable=SC2016
pin_rows P4 "$T/fn-smoke_consoles.sh" 'console_container_args "$app" "$service" "$zones_json" "$net" "$redis_url" "$work/env" > "$args_file" || args_rc=$?'
# shellcheck disable=SC2016
pin_rows P5 "$T/fn-smoke_consoles.sh" 'run_out="$(docker create --name "$name" "${cargs[@]}" "$image" 2>&1)" || run_rc=$?'
# shellcheck disable=SC2016
pin_rows P6 "$T/fn-smoke_consoles.sh" 'elif console_seed_session "$redis_name" "$sid"; then'
# shellcheck disable=SC2016
pin_rows P7 "$T/fn-smoke_consoles.sh" 'console_kernel_route_row "$origin" "$base_path" "$console_path" "$sid" "$name" 3 || ec=1'
# shellcheck disable=SC2016
pin_rows P8 "$T/fn-smoke_consoles.sh" 'console_kernel_control_row "$app" "$image" "$base_path" "$console_path" "$redis_name" "$args_file" "$CONSOLE_KERNEL_LINE" || ec=1'
# shellcheck disable=SC2016
pin_rows P9 "$T/fn-console_kernel_control_row.sh" 'rm_out="$(docker rm -f "$ctl" 2>&1)" || rm_rc=$?'
# shellcheck disable=SC2016
pin_rows P10 "$T/fn-console_kernel_control_row.sh" 'console_kernel_control_probe "$ctl" "$tmp" "$@" || rc=$?'
# shellcheck disable=SC2016
pin_rows P11 "$T/fn-console_kernel_route_row.sh" 'grep -F -q -- "$nonce" "$body" || g_rc=$?'
# shellcheck disable=SC2016
pin_rows P12 "$T/fn-console_kernel_control_probe.sh" 'grep -F -q -- "$kernel_line" "$tmp/ctl.log" || g_rc=$?'
# shellcheck disable=SC2016
pin_rows P13 "$T/fn-console_kernel_control_probe.sh" 'out="$(docker create --name "$ctl" "${cargs[@]}" "$image" 2>&1)" || rc=$?'
# shellcheck disable=SC2016
pin_rows P14 "$T/fn-console_kernel_control_probe.sh" 'if ! console_seed_session "$redis" "$sid"; then'
# shellcheck disable=SC2016
pin_rows P15 "$T/fn-console_smoke_redis_start.sh" 'out="$(docker network create --label paigasus.smoke=console "$network" 2>&1)" || rc=$?'
# shellcheck disable=SC2016
pin_rows P16 "$T/fn-console_smoke_cleanup.sh" 'docker network rm "$CONSOLE_SMOKE_NETWORK" >/dev/null 2>&1 || true'

# --- SMA-671: the workflow pin (W1 rows) ------------------------------------------------------
# W1 keeps the production switch in place (guard-the-guard). It reads images.yml with awk (no YAML
# library) and fails unless each CONSOLE smoke step sets CONSOLE_PARITY_REQUIRED: '1' in its own
# step env, nothing else sets the variable, and the host-build step comes first. A console smoke
# step is a step whose run: calls `ci/images/run.sh all-consoles`, or `ci/images/run.sh smoke`
# with a first argument other than the cargo keys iam and gateway.
W1_WF="$REPO/.github/workflows/images.yml"

# w1_scan <workflow> — one line per step of the steps: list, in file order:
# <index> TAB <console|host|other> TAB <value of CONSOLE_PARITY_REQUIRED in the step env, or -> TAB <name>
# and a last line `stray TAB <n>`: the CONSOLE_PARITY_REQUIRED key lines outside a step env.
w1_scan() {
  awk -v q="'" '
    function flush() {
      if (n > 0) printf "%d\t%s\t%s\t%s\n", n, (console ? "console" : (host ? "host" : "other")), par, name
    }
    {
      line = $0
      if (match(line, /[^ ]/)) { ind = RSTART - 1; body = substr(line, RSTART) } else { ind = -1; body = "" }
      comment = (substr(body, 1, 1) == "#")
    }
    line == "    steps:" { insteps = 1; next }
    insteps && ind == 6 && substr(body, 1, 2) == "- " {
      flush(); n++; console = 0; host = 0; par = "-"; inenv = 0; name = ""
      if (substr(body, 1, 8) == "- name: ") name = substr(body, 9)
      next
    }
    inenv && ind >= 0 && ind < 10 { inenv = 0 }
    n > 0 && ind == 8 && name == "" && substr(body, 1, 6) == "name: " { name = substr(body, 7) }
    n > 0 && ind == 8 && body == "env:" { inenv = 1; next }
    !comment && substr(body, 1, 24) == "CONSOLE_PARITY_REQUIRED:" {
      if (inenv) { v = substr(body, 25); sub(/^ +/, "", v); sub(/ +$/, "", v); par = v } else stray++
      next
    }
    n > 0 && !comment {
      if (index(line, "ci/images/run.sh all-consoles")) console = 1
      if (index(line, "moon run iam-console-ts:build")) host = 1
      p = index(line, "ci/images/run.sh smoke")
      if (p) {
        rest = substr(line, p + 22)
        if (rest == "" || substr(rest, 1, 1) == " ") {
          sub(/^ +/, "", rest); w = rest; sub(/ .*/, "", w); gsub("[\"" q "]", "", w)
          if (w != "" && w != "iam" && w != "gateway") console = 1
        }
      }
    }
    END { flush(); printf "stray\t%d\t-\t-\n", stray + 0 }
  ' "$1"
}

# w1_check <workflow> — rc 0 when the rules above hold; one `W1:` line on stderr per defect.
w1_check() {
  local wf="$1" rc=0 idx kind par name first_idx="" first_name="" host_idx="" host_name="" n_console=0 tab
  tab="$(printf '\t')"
  w1_scan "$wf" > "$T/w1-scan" || { echo "W1: awk could not read ${wf}" >&2; return 1; }
  while IFS="$tab" read -r idx kind par name; do
    if [ "$idx" = stray ]; then
      if [ "$kind" -ne 0 ]; then
        echo "W1: ${kind} CONSOLE_PARITY_REQUIRED line(s) outside a step env (a job or workflow env, or a run body); the Console image self-test step must not inherit the variable." >&2
        rc=1
      fi
      continue
    fi
    if [ "$kind" = console ]; then
      n_console=$((n_console + 1))
      if [ -z "$first_idx" ]; then first_idx="$idx"; first_name="$name"; fi
      if [ "$par" = "-" ]; then
        echo "W1: step '${name}' runs a console smoke, but its own step env has no CONSOLE_PARITY_REQUIRED line." >&2
        rc=1
      elif [ "$par" != "'1'" ]; then
        echo "W1: step '${name}' sets CONSOLE_PARITY_REQUIRED: ${par}; the only accepted line is CONSOLE_PARITY_REQUIRED: '1'." >&2
        rc=1
      fi
    else
      if [ "$kind" = host ] && [ -z "$host_idx" ]; then host_idx="$idx"; host_name="$name"; fi
      if [ "$par" != "-" ]; then
        echo "W1: step '${name}' sets CONSOLE_PARITY_REQUIRED, but it runs no console smoke." >&2
        rc=1
      fi
    fi
  done < "$T/w1-scan"
  if [ "$n_console" -eq 0 ]; then
    echo "W1: found no console smoke step in ${wf##*/} (a step that runs 'ci/images/run.sh all-consoles' or 'ci/images/run.sh smoke <console-key>')." >&2
    return 1
  fi
  if [ -z "$host_idx" ]; then
    echo "W1: no host-build step in ${wf##*/} (no step holds 'moon run iam-console-ts:build')." >&2
    rc=1
  elif [ "$host_idx" -gt "$first_idx" ]; then
    echo "W1: the host-build step '${host_name}' comes after the console smoke step '${first_name}'." >&2
    rc=1
  fi
  if [ "$rc" -eq 0 ]; then
    echo "W1: ${n_console} console smoke step(s) set CONSOLE_PARITY_REQUIRED: '1', after the host-build step '${host_name}'."
  fi
  return "$rc"
}

# w1_mut <row> <awk-program> — images.yml through the program, into $T/<row>.yml. The program sees
# `step` (the name of the current step) and q (a single quote). A copy equal to the original is a
# FAIL row: the mutation did not apply.
w1_mut() {
  awk -v q="'" '/^      - name: / { step = substr($0, 15) } '"$2" "$W1_WF" > "$T/$1.yml" \
    || infra "awk could not mutate images.yml for $1"
  if cmp -s "$W1_WF" "$T/$1.yml"; then say_fail "$1-applied" "the mutation did not apply"; else say_pass "$1-applied"; fi
}

stub_reset
run_fn W1 0 "" "W1:" none w1_check "$W1_WF"
expect_in W1-out "$T/W1.out" "W1: 2 console smoke step(s) set CONSOLE_PARITY_REQUIRED: '1'"
# m1: the env line deleted.
w1_mut W1-m1 '{ l = $0; sub(/^ +/, "", l) } step == "Console release sequence" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { next } { print }'
stub_reset
run_fn W1-m1 1 "step 'Console release sequence' runs a console smoke, but its own step env has no CONSOLE_PARITY_REQUIRED line" "" none w1_check "$T/W1-m1.yml"
# m2: the env line moved to the cargo smoke step.
w1_mut W1-m2 '{ l = $0; sub(/^ +/, "", l) } step == "Build + smoke both consoles" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { next } { print } $0 == "      - name: Smoke each service on its own" { print "        env:"; print "          CONSOLE_PARITY_REQUIRED: " q "1" q }'
stub_reset
run_fn W1-m2 1 "step 'Build + smoke both consoles' runs a console smoke, but its own step env has no|step 'Smoke each service on its own' sets CONSOLE_PARITY_REQUIRED, but it runs no console smoke" "" none w1_check "$T/W1-m2.yml"
# m3: '1' changed to true.
w1_mut W1-m3 '{ l = $0; sub(/^ +/, "", l) } step == "Console release sequence" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { sub(q "1" q, "true") } { print }'
stub_reset
run_fn W1-m3 1 "step 'Console release sequence' sets CONSOLE_PARITY_REQUIRED: true; the only accepted line" "" none w1_check "$T/W1-m3.yml"
# m4: the host-build step moved after the release sequence.
w1_mut W1-m4 '/^      [-#]/ { if (inrel && hold != "") { printf "%s", hold; hold = "" } inrel = 0; inhost = 0 } /^      - name: Host build of both consoles/ { inhost = 1 } inhost { hold = hold $0 "\n"; next } { print } /^      - name: Console release sequence$/ { inrel = 1 }'
stub_reset
run_fn W1-m4 1 "the host-build step 'Host build of both consoles (staged-tree parity reference)' comes after the console smoke step 'Console release sequence'" "" none w1_check "$T/W1-m4.yml"
# m5: the cargo smoke step turned into a console smoke step.
w1_mut W1-m5 '/ci\/images\/run\.sh smoke iam$/ { sub(/smoke iam$/, "smoke iam-console") } { print }'
stub_reset
run_fn W1-m5 1 "step 'Smoke each service on its own' runs a console smoke, but its own step env has no" "" none w1_check "$T/W1-m5.yml"
# m6 (Review Focus 4): the variable at job level, where the self-test step would inherit it.
w1_mut W1-m6 '{ print } $0 == "      PROTO_REPORTER: text" { print "      CONSOLE_PARITY_REQUIRED: " q "1" q }'
stub_reset
run_fn W1-m6 1 "1 CONSOLE_PARITY_REQUIRED line(s) outside a step env" "" none w1_check "$T/W1-m6.yml"
# m7 (Review Focus 4): the env line turned into a comment.
w1_mut W1-m7 '{ l = $0; sub(/^ +/, "", l) } step == "Console release sequence" && l == "CONSOLE_PARITY_REQUIRED: " q "1" q { print "          # CONSOLE_PARITY_REQUIRED: " q "1" q; next } { print }'
stub_reset
run_fn W1-m7 1 "step 'Console release sequence' runs a console smoke, but its own step env has no CONSOLE_PARITY_REQUIRED line" "" none w1_check "$T/W1-m7.yml"
# m8: the host-build step deleted.
w1_mut W1-m8 '/^      [-#]/ { inhost = 0 } /^      - name: Host build of both consoles/ { inhost = 1 } inhost { next } { print }'
stub_reset
run_fn W1-m8 1 "W1: no host-build step in W1-m8.yml" "" none w1_check "$T/W1-m8.yml"
# m9: no console smoke step at all must not read as a pass.
printf '%s\n' 'jobs:' > "$T/W1-m9.yml"
stub_reset
run_fn W1-m9 1 "W1: found no console smoke step in W1-m9.yml" "" none w1_check "$T/W1-m9.yml"

# --- SMA-671: the "Console release sequence" loop (RS rows, Review Focus 2) -------------------
# The step's real run: body, copied out of images.yml, runs under GitHub's own shell flags against
# a stub ci/images/run.sh, syft and uv. A failure on iam-console must still run gateway-console,
# stop the rest of iam-console, and fail the step.
awk 'index($0, "      - name: Console release sequence") == 1 { on = 1; next }
     on && /^        run: \|/ { body = 1; next }
     on && body && /^          / { print substr($0, 11); next }
     on && body && /^ *$/ { print ""; next }
     on && body { exit }
     on && /^      [-#]/ { exit }' "$W1_WF" > "$T/rs-body.sh" || infra "awk could not read images.yml"
[ -s "$T/rs-body.sh" ] || infra "no run: body for 'Console release sequence' in images.yml"
mkdir -p "$T/rs-work/ci/images" "$T/rs-bin"
# shellcheck disable=SC2016 # the stub lines are written literally
printf '%s\n' '#!/usr/bin/env bash' 'printf "%s\n" "$*" >> "$RS_LOG"' \
  'if [ "${1:-}" = "$RS_FAIL_CMD" ] && [ "${2:-}" = "$RS_FAIL_KEY" ]; then exit 1; fi' 'exit 0' \
  > "$T/rs-work/ci/images/run.sh"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$T/rs-bin/syft"
printf '%s\n' '#!/usr/bin/env bash' 'echo "packages=1"' 'exit 0' > "$T/rs-bin/uv"
chmod +x "$T/rs-work/ci/images/run.sh" "$T/rs-bin/syft" "$T/rs-bin/uv"
# rs_case <fail-cmd> <fail-key> <log-name> — the body, with run.sh failing on that one call.
rs_case() {
  RS_FAIL_CMD="$1"; RS_FAIL_KEY="$2"; RS_LOG="$T/rs-log-$3"
  rm -f "$RS_LOG"
  export RS_FAIL_CMD RS_FAIL_KEY RS_LOG
  ( cd "$T/rs-work" && PATH="$T/rs-bin:$PATH" ARCH=amd64 "$BASH" --noprofile --norc -eo pipefail "$T/rs-body.sh" )
}
stub_reset
run_fn RS1 1 "Console release sequence: iam-console failed (rc 1)" "" any rs_case build-oci iam-console RS1
expect_in RS1-gw-build "$T/rs-log-RS1" "build-oci gateway-console out"
expect_in RS1-gw-smoke "$T/rs-log-RS1" "smoke gateway-console"
expect_not_in RS1-iam-stopped "$T/rs-log-RS1" "smoke iam-console"
stub_reset
run_fn RS2 0 "" "::error::" any rs_case none none RS2
expect_in RS2-iam "$T/rs-log-RS2" "smoke iam-console"
expect_in RS2-gw "$T/rs-log-RS2" "smoke gateway-console"
stub_reset
run_fn RS3 1 "Console release sequence: iam-console failed (rc 1)" "" any rs_case smoke iam-console RS3
expect_in RS3-gw "$T/rs-log-RS3" "smoke gateway-console"

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
