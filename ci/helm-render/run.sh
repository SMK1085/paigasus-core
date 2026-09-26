#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:helm-render — the static gate over charts/paigasus (SMA-513 PR 2b).
#
#   run.sh                     checks 1-4 (helm_render.py) against charts/paigasus, then every
#                              charts/paigasus/tests/*.sh (checks 5 and 6)
#   run.sh --self-test         helm_render.py's in-process rows, plus this wrapper's rc rows
#   run.sh --negative-control  each fixture under fixtures/ must fail its own named row
#
# Exit codes: 0 pass | 1 an assertion failed | 2 infrastructure error. They never collapse:
# helm_render.py exits 3 for an assertion and this wrapper maps 3 to 1 and every other non-zero
# code to 2, because a Python traceback exits 1. A chart script's 1 is an assertion; any other
# non-zero code from it (127, 141) is 2. Every row runs after a failure, and the gate's rc is the
# worst one seen: 2 before 1 before 0.
#
# Read-only: everything this gate writes lands under one mktemp -d directory, removed on EXIT,
# except ci/helm-render/.venv, which uv creates and .gitignore ignores. It never writes into
# charts/ and never calls render.sh --update. See README.md.
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no declare -A, no here-string, no pipe into
# an early-exit reader (ci/actionlint check 13), and a "+" guard on every possibly-empty array.
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment, which poisons every $(...) capture of a
# proto or shim call. Exported once here, so every later capture inherits it (SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/helm-render"
CHART="$REPO_ROOT/charts/paigasus"
FIXTURES="$HERE/fixtures"
CHART_SCRIPT_FLOOR=7

# name|rows that must FAIL (";"-separated, exact)|row prefixes that must stay green (";"-separated)
FIXTURE_TABLE=(
  'literal-ingress|1.1 iam|'
  'zones-omits-enabled|1.1 iam+gateway|'
  'leaked-value|2|1.1 '
  'template-only-diff|3a;3a-prime|'
  'slug-mirror|1a|'
  'security-context|4 security-context iam;4 security-context iam+gateway|'
  'app-version-unreleased|8c chart-app-version|'
)

die_infra() { printf 'helm-render: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }

SELFTEST=0
NEGATIVE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        SELFTEST=1; shift ;;
    --negative-control) NEGATIVE=1; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

TMP="$(mktemp -d)" || die_infra "mktemp -d failed"
trap 'rm -rf "$TMP"' EXIT
# helm_render.py's tempfile, and helm's own cache and config, all land under $TMP.
export TMPDIR="$TMP"
export HELM_CACHE_HOME="$TMP/helm-cache" HELM_CONFIG_HOME="$TMP/helm-config" HELM_DATA_HOME="$TMP/helm-data"

# helm, resolved ONCE to an absolute path through proto from the repo root, where the shim finds
# .prototools. No fallback: the golden files are a byte pin of the pinned helm, so another helm
# would give a verdict about the wrong binary. Model: ci/release-parity/ecosystems/release-plz.sh.
resolve_helm() {
  local want got bin
  # Digits and dots only: the [plugins] table also has a `helm = "file://..."` line.
  want="$(sed -n 's/^helm = "\([0-9][0-9.]*\)"$/\1/p' "$REPO_ROOT/.prototools")" || die_infra "cannot read .prototools"
  case "$want" in
    ''|*[!0-9.]*) die_infra "expected one 'helm = \"X.Y.Z\"' pin in .prototools, got: ${want:-<none>}" ;;
  esac
  bin="$(cd "$REPO_ROOT" && proto --reporter text bin helm)" \
    || die_infra "'proto --reporter text bin helm' failed; run 'proto install' from the repo root"
  # -f AND -x: -x alone is true for a searchable directory.
  if [ ! -f "$bin" ] || [ ! -x "$bin" ]; then
    die_infra "helm did not resolve to an executable file. Got: ${bin:-<empty>}"
  fi
  got="$("$bin" version --short)" || die_infra "'$bin version --short' failed"
  case "$got" in
    "v$want"|"v$want+"*) ;;
    *) die_infra "helm is $got but .prototools pins $want; the golden files are a byte pin of the pinned helm" ;;
  esac
  HELM_BIN="$bin"
}

# The venv interpreter, resolved ONCE. --locked stops uv from rewriting uv.lock. After this no uv
# runs, and never from inside a fixture directory (ci/ruff/README.md, the PR 206 lesson).
resolve_python() {
  local py
  command -v uv >/dev/null 2>&1 || die_infra "uv is not on PATH; run 'proto install', or add ~/.proto/shims to PATH"
  py="$(uv run --locked --project "$HERE" python3 -c 'import sys, yaml; print(sys.executable)')" \
    || die_infra "uv could not provide the locked PyYAML environment for $HERE"
  case "$py" in
    "$HERE/.venv/"*) ;;
    *) die_infra "the interpreter is not under $HERE/.venv. Got: ${py:-<empty>}" ;;
  esac
  if [ ! -f "$py" ] || [ ! -x "$py" ]; then
    die_infra "the venv interpreter is not an executable file: $py"
  fi
  PY="$py"
}

module_rc() {  # $1: helm_render.py's raw rc -> the gate's rc
  case "$1" in 0) echo 0 ;; 3) echo 1 ;; *) echo 2 ;; esac
}

script_rc() {  # $1: a chart script's raw rc -> the gate's rc
  case "$1" in 0) echo 0 ;; 1) echo 1 ;; *) echo 2 ;; esac
}

# Runs "$@" (the module, or a stand-in in the self-test) and RETURNS the mapped rc.
mapped_run_module() {
  local raw=0
  "$@" || raw=$?
  return "$(module_rc "$raw")"
}

# Runs one chart script under THIS bash, not its shebang, so the bash 3.2 run covers it.
run_chart_script() {
  local raw=0
  "$BASH" "$1" --set ingress.host=console.example.test || raw=$?
  return "$(script_rc "$raw")"
}

run_chart_scripts() {
  local s worst=0 cs_rc
  local scripts=()
  for s in "$CHART"/tests/*.sh; do
    [ -f "$s" ] && scripts+=("$s")
  done
  if [ "${#scripts[@]}" -lt "$CHART_SCRIPT_FLOOR" ]; then
    printf 'FAIL  [6 floor]: %s chart script(s) under %s, expected at least %s\n' \
      "${#scripts[@]}" "$CHART/tests" "$CHART_SCRIPT_FLOOR" >&2
    return 2
  fi
  for s in "${scripts[@]+"${scripts[@]}"}"; do
    printf -- '--- [6 %s]\n' "${s##*/}"
    cs_rc=0; run_chart_script "$s" || cs_rc=$?
    case "$cs_rc" in
      0) printf 'PASS  [6 %s]\n' "${s##*/}" ;;
      1) printf 'FAIL  [6 %s]: assertion failure\n' "${s##*/}" >&2 ;;
      *) printf 'FAIL  [6 %s]: infrastructure error (rc=2)\n' "${s##*/}" >&2 ;;
    esac
    if [ "$cs_rc" -gt "$worst" ]; then worst="$cs_rc"; fi
  done
  return "$worst"
}

real_run() {
  local worst=0 mod_rc=0 all_rc=0
  mod_rc=0; mapped_run_module "$PY" "$HERE/helm_render.py" --chart "$CHART" || mod_rc=$?
  if [ "$mod_rc" -gt "$worst" ]; then worst="$mod_rc"; fi
  all_rc=0; run_chart_scripts || all_rc=$?
  if [ "$all_rc" -gt "$worst" ]; then worst="$all_rc"; fi
  return "$worst"
}

self_test() {
  local failures=0 got stub st_mod_rc
  _st_row() {  # $1 label, $2 want, $3 got
    if [ "$2" = "$3" ]; then
      printf '  ok %s\n' "$1"
    else
      printf '  FAIL %s: expected %s, got %s\n' "$1" "$2" "$3" >&2
      failures=$((failures + 1))
    fi
  }
  st_mod_rc=0; mapped_run_module "$PY" "$HERE/helm_render.py" --self-test || st_mod_rc=$?
  _st_row "helm_render.py --self-test passes" 0 "$st_mod_rc"
  _st_row "module rc 0 maps to 0" 0 "$(module_rc 0)"
  _st_row "module rc 3 maps to 1" 1 "$(module_rc 3)"
  _st_row "module rc 1 (a traceback) maps to 2" 2 "$(module_rc 1)"
  _st_row "module rc 2 maps to 2" 2 "$(module_rc 2)"
  _st_row "chart-script rc 1 maps to 1" 1 "$(script_rc 1)"
  _st_row "chart-script rc 127 maps to 2" 2 "$(script_rc 127)"
  _st_row "chart-script rc 141 maps to 2" 2 "$(script_rc 141)"
  # The same mappings through the real call paths, with live processes.
  got=0; mapped_run_module "$PY" -c 'import sys; sys.exit(3)' || got=$?
  _st_row "a live module exit 3 reaches the gate as 1" 1 "$got"
  got=0; mapped_run_module "$PY" -c 'raise SystemExit("traceback stand-in")' || got=$?
  _st_row "a live module exit 1 reaches the gate as 2" 2 "$got"
  stub="$TMP/stub-chart-script.sh"
  printf 'exit 127\n' >"$stub"
  got=0; run_chart_script "$stub" || got=$?
  _st_row "a live chart-script rc 127 reaches the gate as 2" 2 "$got"
  printf 'exit 1\n' >"$stub"
  got=0; run_chart_script "$stub" || got=$?
  _st_row "a live chart-script rc 1 reaches the gate as 1" 1 "$got"
  # The worst rc seen: the module's own 2 must reach the caller as 2, not be folded into 1.
  if [ "$st_mod_rc" -eq 2 ]; then return 2; fi
  if [ "$failures" -gt 0 ]; then return 1; fi
  return 0
}

negative_control() {
  local entry name rest fails greens row out fx_rc verdict total
  local nc_failed=0 nc_inconclusive=0 d known
  # The table and the fixture directories must agree: a directory with no row is never run.
  for d in "$FIXTURES"/*/; do
    [ -d "$d" ] || continue
    known=0
    for entry in "${FIXTURE_TABLE[@]}"; do
      if [ "${entry%%|*}" = "$(basename "$d")" ]; then known=1; fi
    done
    if [ "$known" -eq 0 ]; then
      printf 'negative-control FAILED [%s]: fixture directory has no FIXTURE_TABLE row\n' "$(basename "$d")" >&2
      nc_failed=$((nc_failed + 1))
    fi
  done
  for entry in "${FIXTURE_TABLE[@]}"; do
    name="${entry%%|*}"; rest="${entry#*|}"; fails="${rest%%|*}"; greens="${rest#*|}"
    out="$TMP/$name.out"
    rm -rf "$TMP/paigasus"
    if [ ! -d "$FIXTURES/$name" ] || ! cp -R "$CHART" "$TMP/paigasus" || ! cp -R "$FIXTURES/$name/." "$TMP/paigasus/"; then
      printf 'negative-control INCONCLUSIVE: infrastructure error (rc=2) [%s]: cannot build the fixture chart\n' "$name" >&2
      nc_inconclusive=$((nc_inconclusive + 1)); continue
    fi
    fx_rc=0; "$PY" "$HERE/helm_render.py" --chart "$TMP/paigasus" >"$out" 2>&1 || fx_rc=$?
    verdict=OK
    if [ "$fx_rc" != 0 ] && [ "$fx_rc" != 3 ]; then
      verdict="INCONCLUSIVE: infrastructure error (rc=$fx_rc)"
    elif [ "$fx_rc" != 3 ]; then
      verdict="FAILED: the module passed a mutated chart (rc=0)"
    else
      while [ -n "$fails" ]; do
        row="${fails%%;*}"
        if [ "$row" = "$fails" ]; then fails=""; else fails="${fails#*;}"; fi
        if ! grep -qF -- "FAIL  [$row]" "$out"; then verdict="FAILED: named row [$row] did not fail"; fi
      done
      while [ -n "$greens" ]; do
        row="${greens%%;*}"
        if [ "$row" = "$greens" ]; then greens=""; else greens="${greens#*;}"; fi
        if grep -qF -- "FAIL  [$row" "$out"; then verdict="FAILED: a row starting [$row must stay green"; fi
      done
    fi
    total="$(grep -c '^FAIL  \[' "$out" || true)"
    case "$verdict" in
      OK) printf 'negative-control OK [%s]: rc 3, %s row(s) failed, the named row(s) among them\n' "$name" "$total" ;;
      INCONCLUSIVE*) nc_inconclusive=$((nc_inconclusive + 1))
                     printf 'negative-control %s [%s]\n' "$verdict" "$name" >&2; sed 's/^/    /' "$out" >&2 ;;
      *) nc_failed=$((nc_failed + 1))
         printf 'negative-control %s [%s]\n' "$verdict" "$name" >&2; sed 's/^/    /' "$out" >&2 ;;
    esac
  done
  if [ "$nc_failed" -gt 0 ]; then
    printf 'helm-render negative control: %d fixture(s) FAILED\n' "$nc_failed" >&2
    return 1
  elif [ "$nc_inconclusive" -gt 0 ]; then
    printf 'helm-render negative control: %d fixture(s) INCONCLUSIVE\n' "$nc_inconclusive" >&2
    return 2
  fi
  printf '== helm-render negative control passed (%d fixtures) ==\n' "${#FIXTURE_TABLE[@]}"
}

resolve_helm
resolve_python
# The venv first, so the chart scripts' bare python3 gets PyYAML; then the pinned helm.
PATH="$(dirname "$PY"):$(dirname "$HELM_BIN"):$PATH"
export PATH

if [ "$SELFTEST" = 1 ]; then
  st_rc=0; self_test || st_rc=$?
  if [ "$st_rc" -ne 0 ]; then
    printf 'helm-render self-test: FAILED (rc=%s)\n' "$st_rc" >&2
    exit "$st_rc"
  fi
  printf '== helm-render self-test passed ==\n'
  exit 0
fi

if [ "$NEGATIVE" = 1 ]; then
  nc_rc=0; negative_control || nc_rc=$?
  exit "$nc_rc"
fi

real_rc=0; real_run || real_rc=$?
if [ "$real_rc" -eq 0 ]; then
  printf '== helm-render: all checks passed ==\n'
else
  printf '== helm-render: FAILED (rc=%s) ==\n' "$real_rc" >&2
fi
exit "$real_rc"
