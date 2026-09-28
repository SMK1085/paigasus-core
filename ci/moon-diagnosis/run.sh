#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# moon-diagnosis:ok — this file names the moon ci report file on purpose (check 12 of
# repo:actionlint). It runs the CORRECTED procedure, and it holds one broken query that is the
# negative-control input (see OLD_STEP1_LINES below).
#
# repo:moon-diagnosis-exec — runs the root CLAUDE.md moon-diagnosis procedure (Step 1, Step 2 and
# Step 2a) against a real failed `moon ci` report in a throwaway fixture workspace (SMA-714).
#
#   run.sh                     the real run on the working-tree CLAUDE.md
#   run.sh --self-test         fixture tables for the pure units; no moon and no git
#   run.sh --negative-control  the real path on a CLAUDE.md copy that holds the pre-SMA-711 query;
#                              it must fail with exactly the expected row set
#
# Exit codes: 0 pass | 1 an assertion failed | 2 infrastructure error. Every rc 2 message holds
# the text `infrastructure error (rc=2)`.
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no heredoc,
# no pipe into an early-exit reader (ci/actionlint check 13), and a "+" guard on every
# possibly-empty array. Everything it writes lands under one mktemp -d directory, removed on EXIT.
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment. This script does not capture proto
# output today; the export covers a later capture (standing rule, SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/moon-diagnosis"
REPORT_REL=".moon/cache/ciReport.json"
SELF_TEST_CASES=37
FAILED_TASKS="fail3 multi missing slow"

die_infra() { printf 'moon-diagnosis-exec: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }

SELFTEST=0
NEGATIVE=0
CLAUDE_MD="$REPO_ROOT/CLAUDE.md"
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        SELFTEST=1; shift ;;
    --negative-control) NEGATIVE=1; shift ;;
    --claude-md)        [ $# -ge 2 ] || die_infra "--claude-md needs a path"; CLAUDE_MD="$2"; shift 2 ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

TMP="$(mktemp -d)" || die_infra "mktemp -d failed"
trap 'rm -rf "$TMP"' EXIT

# moon-diagnosis:ok — NEGATIVE-CONTROL INPUT, BROKEN ON PURPOSE. This is the pre-SMA-711 Step 1
# query (git show 81451a82^:CLAUDE.md). It reads command and exitCode from the operation, not
# from .meta, so it prints null. Never copy it into documentation.
OLD_STEP1_LINES=(
  "jq '.actions[] | select(.status==\"failed\")"
  "    | {label, error,"
  "       exec: (.operations[] | select(.meta.type==\"task-execution\") | {command, exitCode})}' \\"
  "   $REPORT_REL"
)

# The exact row set that the old query must produce (step1_verdict rows, sorted).
NEGATIVE_WANT_ROWS=(
  "bad-exec RunTask(probe:fail3) object"
  "bad-exec RunTask(probe:missing) object"
  "bad-exec RunTask(probe:multi) object"
  "bad-exec RunTask(probe:slow) object"
)

# ---------------------------------------------------------------------------------------------
# Unit 1. Prints one verdict word: ok | no-block | no-step1 | no-fence | unclosed-fence |
# multiple-step1. On ok, OUT holds the fence body with the common leading indent removed.
extract_step1_query() {
  local md="$1" out="$2"
  [ -f "$md" ] && [ -r "$md" ] || { echo "no-block"; return 0; }
  awk -v out="$out" '
    function lead(s) { match(s, /^[ \t]*/); return RLENGTH }
    index($0, "<!-- moon-diagnosis:begin -->") { nb++; if (nb == 1) { inblk = 1; next } }
    index($0, "<!-- moon-diagnosis:end -->")   { ne++; if (inblk) { inblk = 0; done = 1 } next }
    !inblk { next }
    {
      if (state == 0) { if (index($0, "Step 1")) state = 1; next }
      if (state == 1) {
        if ($0 ~ /^[ \t]*```bash[ \t]*$/) { state = 2; n = 0; next }
        if (index($0, "**Step ")) { state = 9 }
        next
      }
      if (state == 2) {
        if ($0 ~ /^[ \t]*```[ \t]*$/) { state = 3; next }
        body[++n] = $0; next
      }
      if (state == 3) {
        if (index($0, "**Step 1")) { multi = 1 }
        if (index($0, "**Step 2")) { state = 4 }
        else if ($0 ~ /^[ \t]*```bash[ \t]*$/) { multi = 1 }
        next
      }
      if (state == 4) { if (index($0, "**Step 1")) multi = 1; next }
    }
    END {
      if (nb != 1 || ne != 1 || !done) { print "no-block"; exit 0 }
      if (state == 0) { print "no-step1"; exit 0 }
      if (state == 1 || state == 9) { print "no-fence"; exit 0 }
      if (state == 2) { print "unclosed-fence"; exit 0 }
      if (multi) { print "multiple-step1"; exit 0 }
      min = -1
      for (i = 1; i <= n; i++) {
        if (body[i] ~ /^[ \t]*$/) continue
        l = lead(body[i]); if (min < 0 || l < min) min = l
      }
      if (min < 0) min = 0
      printf "" > out
      for (i = 1; i <= n; i++) print substr(body[i], min + 1) >> out
      close(out)
      print "ok"
    }
  ' "$md"
}

# Unit 2. Prints: ok | bad-shape <reason>. On ok, PROG_OUT holds the jq program and
# PROG_OUT.flags holds one flag per line. Pure bash: no subprocess reads the doc text.
query_shape_verdict() {
  local qf="$1" pout="$2" text rest tok prog after nl
  nl='
'
  [ -f "$qf" ] || { echo "bad-shape no-query-file"; return 0; }
  text="$(cat "$qf")" || { echo "verdict-error shape rc=$?"; return 0; }
  text="${text//\\$nl/ }"
  rest="${text#"${text%%[![:space:]]*}"}"
  case "$rest" in
    "jq "*|"jq	"*) rest="${rest#jq}" ;;
    *) echo "bad-shape not-jq"; return 0 ;;
  esac
  : > "$pout.flags"
  while :; do
    rest="${rest#"${rest%%[![:space:]]*}"}"
    case "$rest" in
      -*) tok="${rest%%[[:space:]]*}"
          case "$tok" in
            -r|-c|-s|-e) printf '%s\n' "$tok" >> "$pout.flags"; rest="${rest#"$tok"}" ;;
            *) echo "bad-shape flag-not-allowed $tok"; return 0 ;;
          esac ;;
      *) break ;;
    esac
  done
  case "$rest" in
    "'"*) rest="${rest#\'}" ;;
    *) echo "bad-shape no-single-quoted-program"; return 0 ;;
  esac
  case "$rest" in
    *"'"*) ;;
    *) echo "bad-shape unclosed-program"; return 0 ;;
  esac
  prog="${rest%%\'*}"
  after="${rest#*\'}"
  after="${after#"${after%%[![:space:]]*}"}"
  after="${after%"${after##*[![:space:]]}"}"
  [ "$after" = "$REPORT_REL" ] || { echo "bad-shape tail-is-not-the-report-path"; return 0; }
  [ -n "$prog" ] || { echo "bad-shape empty-program"; return 0; }
  printf '%s' "$prog" > "$pout"
  echo "ok"
}

# Unit 3. Makes the fixture workspace in DIR and prints the sha of commit 1.
make_fixture() {
  local dir="$1" pin="$2"
  mkdir -p "$dir/probe" "$dir/.moon" || return 2
  printf 'moon = "%s"\n' "$pin" > "$dir/.prototools"
  printf '%s\n' \
    "projects:" \
    "  probe: 'probe'" \
    "vcs:" \
    "  client: 'git'" \
    "  defaultBranch: 'main'" > "$dir/.moon/workspace.yml"
  printf '.moon/cache\n' > "$dir/.gitignore"
  printf 'one\n' > "$dir/probe/file.txt"
  printf '%s\n' \
    "tasks:" \
    "  fail3:" \
    "    script: 'exit 3'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" \
    "  multi:" \
    "    script: |" \
    "      echo a" \
    "      exit 4" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" \
    "  missing:" \
    "    command: 'no-such-binary-xyz'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" \
    "  slow:" \
    "    command: 'sleep 20'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false, timeout: 2 }" \
    "  ok:" \
    "    command: 'true'" \
    "    toolchain: 'system'" \
    "    inputs: ['file.txt']" \
    "    options: { cache: false }" > "$dir/probe/moon.yml"
  fx_git -C "$dir" init -q -b main || return 2
  fx_git -C "$dir" config commit.gpgsign false || return 2
  fx_git -C "$dir" config tag.gpgsign false || return 2
  fx_git -C "$dir" config maintenance.auto false || return 2
  fx_git -C "$dir" config gc.auto 0 || return 2
  fx_git -C "$dir" config user.name moon-diagnosis-exec || return 2
  fx_git -C "$dir" config user.email moon-diagnosis-exec@example.invalid || return 2
  fx_git -C "$dir" add -A || return 2
  fx_git -C "$dir" commit -q -m one || return 2
  fx_git -C "$dir" rev-parse HEAD || return 2
  printf 'two\n' > "$dir/probe/file.txt"
  fx_git -C "$dir" add -A || return 2
  fx_git -C "$dir" commit -q -m two || return 2
}

# Every fixture git call ignores the host's global and system config (signing, hooks, identity).
fx_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git "$@"; }

outer_report_hash() {
  if [ -f "$REPO_ROOT/$REPORT_REL" ]; then cksum < "$REPO_ROOT/$REPORT_REL"; else echo absent; fi
}

# Unit 4. Runs the nested moon in DIR with an ALLOWLIST environment. Exits 2 on every
# infrastructure fault. Returns 0 when the fixture report is valid and shows the designed run.
run_nested_moon() {
  local dir="$1" base="$2" pin="$3" h_before h_after got nm_rc fx_rc
  got="$(cd "$dir" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$dir" moon --version 2>&1)" \
    || die_infra "'moon --version' failed in the fixture (pin $pin): $got"
  [ "$got" = "moon $pin" ] || die_infra "the fixture moon is '$got', expected 'moon $pin'"
  h_before="$(outer_report_hash)"
  nm_rc=0; (cd "$dir" && env -i HOME="$HOME" PATH="$PATH" TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-C}" PROTO_HOME="${PROTO_HOME:-$HOME/.proto}" PROTO_REPORTER=text MOON_WORKSPACE_ROOT="$dir" moon ci probe:fail3 probe:multi probe:missing probe:slow probe:ok --base "$base") >"$TMP/nested.out" 2>"$TMP/nested.err" || nm_rc=$?
  h_after="$(outer_report_hash)"
  [ "$h_before" = "$h_after" ] || die_infra "the nested run wrote to the real workspace report ($REPO_ROOT/$REPORT_REL)"
  [ "$nm_rc" -ne 0 ] || die_infra "the nested moon ci exited 0; the fixture did not fail, so the report proves nothing"
  if ! jq -e . "$dir/$REPORT_REL" >/dev/null 2>&1; then
    if grep -qF -- "proto-shim" "$TMP/nested.err"; then
      die_infra "no valid fixture report; the nested moon hit a proto-shim error (ci/CLAUDE.md, SMA-592): $(cat "$TMP/nested.err")"
    fi
    die_infra "no valid fixture report after the nested moon ci (rc=$nm_rc): $(cat "$TMP/nested.err")"
  fi
  # Spec M5: under inherited CI/GITHUB_* variables moon printed `Base revision: N/A` and ignored
  # --base. The allowlist removes them; this line proves it on every run.
  grep -qF -- "Base revision: $base" "$TMP/nested.out" || die_infra "the nested moon ci did not use --base $base (CI or GITHUB_* variables leaked?)"
  fx_rc=0; jq -e "$FIXTURE_RUN_CHECK" "$dir/$REPORT_REL" >/dev/null || fx_rc=$?
  [ "$fx_rc" -eq 0 ] || die_infra "the fixture did not run as designed (fixture-run check rc=$fx_rc); see $dir/$REPORT_REL"
}

# The gate's OWN jq check that the fixture ran as designed. Measured on moon 2.5.3, macOS and
# Linux: the slow task's ACTION status is `failed`; only its task-execution operation is
# `timed-out`.
FIXTURE_RUN_CHECK='[.actions[] | select((.label // "") | startswith("RunTask(probe:")) | "\(.label)=\(.status)"] | sort
  == ["RunTask(probe:fail3)=failed", "RunTask(probe:missing)=failed", "RunTask(probe:multi)=failed",
      "RunTask(probe:ok)=passed", "RunTask(probe:slow)=failed"]'

# Unit 5. Runs the documented query with jq directly. Prints nothing on success, or one row.
run_step1_query() {
  local pf="$1" ff="$2" dir="$3" out="$4" f q_rc
  local flags=()
  while IFS= read -r f; do flags+=("$f"); done < "$ff"
  q_rc=0; (cd "$dir" && jq ${flags[@]+"${flags[@]}"} "$(cat "$pf")" "$REPORT_REL") >"$out" 2>"$out.err" || q_rc=$?
  [ "$q_rc" -eq 0 ] || echo "query-error rc=$q_rc"
}

# Unit 6. One row per violation, nothing when clean. Rows are sorted.
STEP1_VERDICT_JQ='
def expected: {
  "RunTask(probe:fail3)":   {status: "failed",    exitCode: 3,    command: "exit 3"},
  "RunTask(probe:multi)":   {status: "failed",    exitCode: 4,    command: "echo a exit 4"},
  "RunTask(probe:missing)": {status: "failed",    exitCode: 127,  command: "no-such-binary-xyz"},
  "RunTask(probe:slow)":    {status: "timed-out", exitCode: null, command: "sleep 20"}
};
if length == 0 then "empty-output"
else
  . as $rows
  | [ ( expected | to_entries[] | .key as $l | .value as $want
        | ($rows | map(select(type == "object" and .label == $l))) as $m
        | if ($m | length) == 0 then "missing-row \($l)"
          else $m[]
            | (.exec | type) as $t
            | if $t != "array" then "bad-exec \($l) \(if has("exec") then $t else "absent" end)"
              elif any(.exec[]; type == "object" and .status == $want.status
                                and .exitCode == $want.exitCode and .command == $want.command)
              then empty
              else "wrong-exec \($l)" end
          end ),
      ( $rows[]
        | (if type == "object" then (.label // "<none>") else "<non-object>" end) as $l
        | select(expected | has($l) | not)
        | "unexpected-row \($l)" ) ]
  | sort | .[]
end'

step1_verdict() {
  local out="$1" v_rc
  v_rc=0; jq -r -s "$STEP1_VERDICT_JQ" "$out" >"$TMP/step1-verdict.rows" 2>"$TMP/step1-verdict.err" || v_rc=$?
  if [ "$v_rc" -ne 0 ]; then echo "verdict-error step1 rc=$v_rc"; return 0; fi
  cat "$TMP/step1-verdict.rows"
}

# Unit 7, Step 2a half. Integer microseconds: a float compare would red a difference of exactly
# 1000 ms. finishedAt is a naive UTC ISO time; lastRunTime is epoch milliseconds (spec M7).
STEP2A_PY='
import datetime, json, sys
d, rel = sys.argv[1], sys.argv[2]
with open(d + "/" + rel) as fh:
    rep = json.load(fh)
fin = {a.get("label"): a.get("finishedAt") for a in rep.get("actions", [])}
epoch = datetime.datetime(1970, 1, 1, tzinfo=datetime.timezone.utc)
for t in sys.argv[3:]:
    label = "RunTask(probe:" + t + ")"
    if not fin.get(label):
        print("step2a-no-action " + t)
        continue
    try:
        with open(d + "/.moon/cache/states/probe/" + t + "/lastRun.json") as fh:
            lr = int(json.load(fh)["lastRunTime"])
    except (OSError, ValueError, KeyError, TypeError):
        print("step2a-no-lastrun " + t)
        continue
    f = datetime.datetime.fromisoformat(fin[label]).replace(tzinfo=datetime.timezone.utc)
    diff_us = (f - epoch) // datetime.timedelta(microseconds=1) - lr * 1000
    if not 0 <= diff_us <= 1000000:
        print("step2a-out-of-range %s %.3f" % (t, diff_us / 1000))
'

step2_verdict() {
  local dir="$1" t f s="$1/.moon/cache/states/probe" p_rc
  for t in $FAILED_TASKS; do
    for f in stdout.log stderr.log lastRun.json; do
      [ -f "$s/$t/$f" ] || echo "step2-missing-file $t/$f"
    done
  done
  if [ -f "$s/missing/stderr.log" ] && ! grep -qF -- "command not found" "$s/missing/stderr.log"; then
    echo "step2-missing-no-command-not-found"
  fi
  if [ -f "$s/multi/stdout.log" ] && ! grep -qx -- "a" "$s/multi/stdout.log"; then
    echo "step2-multi-no-line-a"
  fi
  # shellcheck disable=SC2086 # FAILED_TASKS is a fixed list of four words.
  p_rc=0; python3 -c "$STEP2A_PY" "$dir" "$REPORT_REL" $FAILED_TASKS >"$TMP/step2a.rows" 2>"$TMP/step2a.err" || p_rc=$?
  if [ "$p_rc" -ne 0 ]; then echo "verdict-error step2 rc=$p_rc"; return 0; fi
  cat "$TMP/step2a.rows"
}

# Negative-control helper: writes a copy of MD in which the Step 1 fence body is BODY_FILE,
# indented like the fence. Prints ok, or a word when it cannot find the fence.
swap_step1_body() {
  local md="$1" body="$2" out="$3"
  awk -v bodyf="$body" '
    index($0, "<!-- moon-diagnosis:begin -->") { inblk = 1 }
    index($0, "<!-- moon-diagnosis:end -->")   { inblk = 0 }
    {
      if (inblk && state == 0 && index($0, "Step 1")) { state = 1; print; next }
      if (state == 1 && $0 ~ /^[ \t]*```bash[ \t]*$/) {
        print; match($0, /^[ \t]*/); ind = substr($0, 1, RLENGTH)
        while ((getline l < bodyf) > 0) print ind l
        state = 2; next
      }
      if (state == 2) { if ($0 ~ /^[ \t]*```[ \t]*$/) { state = 3; print }; next }
      print
    }
    END { if (state == 3) print "ok" > "/dev/stderr"; else print "no-fence" > "/dev/stderr" }
  ' "$md" > "$out" 2>"$out.verdict"
  cat "$out.verdict"
}

# ---------------------------------------------------------------------------------------------
# Self-test. Each case asserts the exact verdict text; the case count is asserted at the end.
ST_N=0
ST_FAIL=0
st_expect() {
  local name="$1" want="$2" got="$3"
  ST_N=$((ST_N + 1))
  if [ "$got" = "$want" ]; then
    printf 'PASS  [self-test] %s\n' "$name"
  else
    printf 'FAIL  [self-test] %s\n        want: %s\n        got:  %s\n' "$name" "$want" "$got"
    ST_FAIL=$((ST_FAIL + 1))
  fi
}

# Joins a verdict's rows with ";" so a case compares one string.
rows_of() { local r out=""; while IFS= read -r r; do out="${out:+$out;}$r"; done < "$1"; printf '%s' "$out"; }

st_md() {
  # $1 file, then lines
  local f="$1"; shift
  printf '%s\n' "$@" > "$f"
}

self_test() {
  local d="$TMP/st" q="$TMP/st/q" p="$TMP/st/p" o="$TMP/st/o" r="$TMP/st/r"
  mkdir -p "$d"
  local B='  <!-- moon-diagnosis:begin -->' E='  <!-- moon-diagnosis:end -->'
  local S1='  **Step 1 — which task, what command, what exit code.**'
  local S2='  **Step 2 — why.** `cat` the logs.'
  local FO='  ```bash' FC='  ```'

  # --- unit 1
  st_md "$d/md1" "x" "$B" "$S1" "$FO" "  jq '.a' \\" "     $REPORT_REL" "$FC" "  prose" "$S2" "$E"
  st_expect "u1 current shape" "ok" "$(extract_step1_query "$d/md1" "$q")"
  st_expect "u1 current shape dedented" "jq '.a' \\;   $REPORT_REL" "$(rows_of "$q")"
  st_md "$d/md2" "x" "$S1" "$FO" "  jq ." "$FC"
  st_expect "u1 no block" "no-block" "$(extract_step1_query "$d/md2" "$q")"
  st_md "$d/md3" "$B" "$S2" "$FO" "  jq ." "$FC" "$E"
  st_expect "u1 no Step 1" "no-step1" "$(extract_step1_query "$d/md3" "$q")"
  st_md "$d/md4" "$B" "$S1" "  prose only" "$S2" "$FO" "  jq ." "$FC" "$E"
  st_expect "u1 Step 1 with no fence" "no-fence" "$(extract_step1_query "$d/md4" "$q")"
  st_md "$d/md5" "$B" "$S1" "$FO" "  jq ." "$E"
  st_expect "u1 unclosed fence" "unclosed-fence" "$(extract_step1_query "$d/md5" "$q")"
  st_md "$d/md6" "$B" "$S1" "$FO" "  jq ." "$FC" "$FO" "  jq .b" "$FC" "$S2" "$E"
  st_expect "u1 two Step 1 fences" "multiple-step1" "$(extract_step1_query "$d/md6" "$q")"
  st_md "$d/md7" "$B" "$S1" "$FO" "  jq ." "$FC" "$S2" "$S1" "$E"
  st_expect "u1 second Step 1 heading" "multiple-step1" "$(extract_step1_query "$d/md7" "$q")"
  st_md "$d/md8" "$B" "$B" "$S1" "$FO" "  jq ." "$FC" "$E"
  st_expect "u1 two begin markers" "no-block" "$(extract_step1_query "$d/md8" "$q")"

  # --- unit 2
  printf '%s\n' "jq '.actions[] | select(.status==\"failed\")" \
    "    | {label, error," \
    "       exec: [.operations[] | select(.meta.type==\"task-execution\")" \
    "              | {status, exitCode: .meta.exitCode, command: .meta.command}]}' \\" \
    "   $REPORT_REL" > "$q"
  st_expect "u2 current query" "ok" "$(query_shape_verdict "$q" "$p")"
  st_expect "u2 current query flags" "" "$(cat "$p.flags")"
  printf '%s\n' "${OLD_STEP1_LINES[@]}" > "$q"
  st_expect "u2 pre-SMA-711 query" "ok" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq -r -c '.a' $REPORT_REL" > "$q"
  st_expect "u2 allowed flags" "ok" "$(query_shape_verdict "$q" "$p")"
  st_expect "u2 allowed flags file" "-r;-c" "$(rows_of "$p.flags")"
  printf '%s\n' "jq '.a' $REPORT_REL | head" > "$q"
  st_expect "u2 pipe after path" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq '.a' $REPORT_REL > out" > "$q"
  st_expect "u2 redirect after path" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq --arg x y '.a' $REPORT_REL" > "$q"
  st_expect "u2 flag not allowed" "bad-shape flag-not-allowed --arg" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq '.a' .moon/cache/runReport.json" > "$q"
  st_expect "u2 another file" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "jq '.a' '.b' $REPORT_REL" > "$q"
  st_expect "u2 quote in program" "bad-shape tail-is-not-the-report-path" "$(query_shape_verdict "$q" "$p")"
  printf '%s\n' "cat $REPORT_REL" > "$q"
  st_expect "u2 not jq" "bad-shape not-jq" "$(query_shape_verdict "$q" "$p")"

  # --- unit 6
  printf '%s\n' \
    '{"label":"RunTask(probe:fail3)","error":"e","exec":[{"status":"failed","exitCode":3,"command":"exit 3"}]}' \
    '{"label":"RunTask(probe:missing)","error":"e","exec":[{"status":"failed","exitCode":127,"command":"no-such-binary-xyz"}]}' \
    '{"label":"RunTask(probe:multi)","error":"e","exec":[{"status":"failed","exitCode":4,"command":"echo a exit 4"}]}' \
    '{"label":"RunTask(probe:slow)","error":"e","exec":[{"status":"timed-out","exitCode":null,"command":"sleep 20"}]}' > "$o.good"
  step1_verdict "$o.good" > "$r"
  st_expect "u6 correct output" "" "$(rows_of "$r")"
  printf '%s\n' \
    '{"label":"RunTask(probe:fail3)","error":"e","exec":{"command":null,"exitCode":null}}' \
    '{"label":"RunTask(probe:missing)","error":"e","exec":{"command":null,"exitCode":null}}' \
    '{"label":"RunTask(probe:multi)","error":"e","exec":{"command":null,"exitCode":null}}' \
    '{"label":"RunTask(probe:slow)","error":"e","exec":{"command":null,"exitCode":null}}' > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 old null output" "bad-exec RunTask(probe:fail3) object;bad-exec RunTask(probe:missing) object;bad-exec RunTask(probe:multi) object;bad-exec RunTask(probe:slow) object" "$(rows_of "$r")"
  : > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 empty output" "empty-output" "$(rows_of "$r")"
  { cat "$o.good"; printf '%s\n' '{"label":"RunTask(probe:ok)","exec":[]}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 ok row present" "unexpected-row RunTask(probe:ok)" "$(rows_of "$r")"
  grep -vF 'probe:multi' "$o.good" > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 one row missing" "missing-row RunTask(probe:multi)" "$(rows_of "$r")"
  { grep -vF 'probe:slow' "$o.good"; printf '%s\n' '{"label":"RunTask(probe:slow)","exec":null}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 exec null" "bad-exec RunTask(probe:slow) null" "$(rows_of "$r")"
  { grep -vF 'probe:slow' "$o.good"; printf '%s\n' '{"label":"RunTask(probe:slow)"}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 exec absent" "bad-exec RunTask(probe:slow) absent" "$(rows_of "$r")"
  { grep -vF 'probe:fail3' "$o.good"; printf '%s\n' '{"label":"RunTask(probe:fail3)","exec":[{"exitCode":3,"command":"exit 3"}]}'; } > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 status dropped" "wrong-exec RunTask(probe:fail3)" "$(rows_of "$r")"
  printf '%s\n' 'this is not json' > "$o"
  step1_verdict "$o" > "$r"
  st_expect "u6 verdict tool fails" "verdict-error step1 rc=N" "$(rows_of "$r" | sed 's/rc=[1-9][0-9]*$/rc=N/')"

  # --- unit 7 (fixture report and state files; finishedAt 2026-09-27T14:49:28.972000 is
  # 1790520568972000 us after the epoch)
  local diff want
  for diff in 0 1000 1001 -1; do
    st_step2_fixture "$d/s2" "$((1790520568972 - diff))"
    case "$diff" in
      0|1000) want="" ;;
      *) want="step2a-out-of-range fail3 $diff.000;step2a-out-of-range multi $diff.000;step2a-out-of-range missing $diff.000;step2a-out-of-range slow $diff.000" ;;
    esac
    step2_verdict "$d/s2" > "$r"
    st_expect "u7 Step 2a difference $diff ms" "$want" "$(rows_of "$r")"
  done
  st_step2_fixture "$d/s2" 1790520568972
  rm -f "$d/s2/.moon/cache/states/probe/multi/stderr.log"
  step2_verdict "$d/s2" > "$r"
  st_expect "u7 missing stderr.log" "step2-missing-file multi/stderr.log" "$(rows_of "$r")"
  st_step2_fixture "$d/s2" 1790520568972
  printf 'something else\n' > "$d/s2/.moon/cache/states/probe/missing/stderr.log"
  step2_verdict "$d/s2" > "$r"
  st_expect "u7 no command not found" "step2-missing-no-command-not-found" "$(rows_of "$r")"

  # --- negative-control helper
  printf '%s\n' "${OLD_STEP1_LINES[@]}" > "$d/old"
  st_expect "swap Step 1 body" "ok" "$(swap_step1_body "$d/md1" "$d/old" "$d/md1.swapped")"
  extract_step1_query "$d/md1.swapped" "$q" > /dev/null
  st_expect "swap round-trips through unit 1" "$(rows_of "$d/old")" "$(rows_of "$q")"

  local n="$ST_N"
  st_expect "self-test case count" "$SELF_TEST_CASES" "$n"
  if [ "$ST_FAIL" -ne 0 ]; then
    printf 'moon-diagnosis-exec self-test: %d case(s) FAILED\n' "$ST_FAIL" >&2
    return 1
  fi
  printf 'moon-diagnosis-exec self-test: all %d cases passed\n' "$ST_N"
}

# Writes a Step 2 fixture under DIR: a report with finishedAt 2026-09-27T14:49:28.972000 for each
# failed task, and state files whose lastRun.json holds LASTRUN.
st_step2_fixture() {
  local dir="$1" lastrun="$2" t
  rm -rf "$dir"
  mkdir -p "$dir/.moon/cache/states/probe"
  printf '{"actions":[%s]}\n' \
    '{"label":"RunTask(probe:fail3)","finishedAt":"2026-09-27T14:49:28.972000"},{"label":"RunTask(probe:multi)","finishedAt":"2026-09-27T14:49:28.972000"},{"label":"RunTask(probe:missing)","finishedAt":"2026-09-27T14:49:28.972000"},{"label":"RunTask(probe:slow)","finishedAt":"2026-09-27T14:49:28.972000"}' \
    > "$dir/$REPORT_REL"
  for t in $FAILED_TASKS; do
    mkdir -p "$dir/.moon/cache/states/probe/$t"
    : > "$dir/.moon/cache/states/probe/$t/stdout.log"
    : > "$dir/.moon/cache/states/probe/$t/stderr.log"
    printf '{"exitCode":1,"lastRunTime":%s}\n' "$lastrun" > "$dir/.moon/cache/states/probe/$t/lastRun.json"
  done
  printf 'a\n' > "$dir/.moon/cache/states/probe/multi/stdout.log"
  printf 'bash: line 1: no-such-binary-xyz: command not found\n' > "$dir/.moon/cache/states/probe/missing/stderr.log"
}

# ---------------------------------------------------------------------------------------------
ROWS=0
emit() { printf 'ROW %s\n' "$1"; ROWS=$((ROWS + 1)); }
emit_file() { local row; while IFS= read -r row; do [ -n "$row" ] && emit "$row"; done < "$1"; }

preflight() {
  local tool
  for tool in moon jq git python3; do
    command -v "$tool" >/dev/null 2>&1 || die_infra "'$tool' is not on PATH"
  done
}

repo_moon_pin() {
  local pin
  pin="$(sed -n 's/^moon = "\([0-9][0-9.]*\)"$/\1/p' "$REPO_ROOT/.prototools")" || die_infra "cannot read .prototools"
  case "$pin" in
    ''|*[!0-9.]*) die_infra "expected one 'moon = \"X.Y.Z\"' pin in .prototools, got: ${pin:-<none>}" ;;
  esac
  printf '%s' "$pin"
}

real_run() {
  local v pin base fx="$TMP/fixture"
  preflight
  v="$(extract_step1_query "$CLAUDE_MD" "$TMP/query")" || die_infra "extract_step1_query crashed"
  if [ "$v" != "ok" ]; then emit "extract $v"; return 1; fi
  v="$(query_shape_verdict "$TMP/query" "$TMP/prog")" || die_infra "query_shape_verdict crashed"
  if [ "$v" != "ok" ]; then emit "$v"; return 1; fi
  pin="$(repo_moon_pin)"
  base="$(make_fixture "$fx" "$pin")" || die_infra "make_fixture failed"
  run_nested_moon "$fx" "$base" "$pin"
  run_step1_query "$TMP/prog" "$TMP/prog.flags" "$fx" "$TMP/step1.out" > "$TMP/step1-query.rows" || die_infra "run_step1_query crashed"
  emit_file "$TMP/step1-query.rows"
  if [ "$ROWS" -eq 0 ]; then
    step1_verdict "$TMP/step1.out" > "$TMP/step1.rows" || die_infra "step1_verdict crashed"
    emit_file "$TMP/step1.rows"
  fi
  step2_verdict "$fx" > "$TMP/step2.rows" || die_infra "step2_verdict crashed"
  emit_file "$TMP/step2.rows"
  if [ "$ROWS" -ne 0 ]; then
    printf 'moon-diagnosis-exec: %d row(s) FAILED; the documented procedure does not match a real report\n' "$ROWS" >&2
    return 1
  fi
  printf 'moon-diagnosis-exec: Step 1, Step 2 and Step 2a hold against a real failed moon ci report (moon %s)\n' "$pin"
}

negative_control() {
  local ctl_md="$TMP/ctl-CLAUDE.md" ctl_out="$TMP/ctl.out" v nc_rc
  printf '%s\n' "${OLD_STEP1_LINES[@]}" > "$TMP/old-query"
  v="$(swap_step1_body "$CLAUDE_MD" "$TMP/old-query" "$ctl_md")"
  [ "$v" = "ok" ] || die_infra "negative control: cannot find the Step 1 fence in $CLAUDE_MD ($v)"
  nc_rc=0; "$BASH" "$HERE/run.sh" --claude-md "$ctl_md" >"$ctl_out" 2>&1 || nc_rc=$?
  sed -n 's/^ROW //p' "$ctl_out" | LC_ALL=C sort > "$TMP/ctl.rows"
  printf '%s\n' "${NEGATIVE_WANT_ROWS[@]}" | LC_ALL=C sort > "$TMP/ctl.want"
  case "$nc_rc" in
    0) printf 'moon-diagnosis-exec negative control: FAILED, the gate passed the pre-SMA-711 query\n' >&2; return 1 ;;
    1) ;;
    *) printf 'moon-diagnosis-exec negative control: infrastructure error (rc=2): the child exited %s\n' "$nc_rc" >&2; cat "$ctl_out" >&2; return 2 ;;
  esac
  if ! cmp -s "$TMP/ctl.rows" "$TMP/ctl.want"; then
    printf 'moon-diagnosis-exec negative control: FAILED, the row set differs from the expected set\n' >&2
    printf 'want:\n' >&2; cat "$TMP/ctl.want" >&2
    printf 'got:\n' >&2; cat "$TMP/ctl.rows" >&2
    return 1
  fi
  printf 'moon-diagnosis-exec negative control: passed, the pre-SMA-711 query reds with exactly %d rows\n' "${#NEGATIVE_WANT_ROWS[@]}"
}

if [ "$SELFTEST" = 1 ]; then
  st_rc=0; self_test || st_rc=$?
  exit "$st_rc"
fi
if [ "$NEGATIVE" = 1 ]; then
  nc_rc=0; negative_control || nc_rc=$?
  exit "$nc_rc"
fi
real_rc=0; real_run || real_rc=$?
exit "$real_rc"
