#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# OSV advisory gate for the npm and pip lockfiles (SMA-518).
#
# `rs/` is deliberately NOT scanned here: repo:deny (cargo-deny) already gates it and also
# enforces license + yanked-crate policy that OSV does not, so folding Rust in would lose
# coverage rather than add it.
#
# WHY THIS SCRIPT EXISTS RATHER THAN A BARE `osv-scanner` COMMAND — osv-scanner's own exit
# codes are necessary but NOT sufficient:
#
#   0   scanned, no vulnerabilities
#   1   vulnerabilities found
#   127 path could not be resolved (e.g. a lockfile was renamed/moved)
#   128 NO package sources found *in aggregate*
#
# The trap is 128's "in aggregate". A run where ts/pnpm-lock.yaml yields 787 packages and
# py/uv.lock silently yields 0 — a renamed format, an extractor regression, a truncated
# file — exits **0**, and "no vulnerabilities" becomes indistinguishable from "scanned
# nothing". Verified against osv-scanner 2.5.0. So this script asserts a per-lockfile
# package count as a control, the on-disk equivalent of the control series the Prometheus
# rule fixtures carry.
#
# The counts come from stderr ("Scanned <path> file and found N packages"), which is not a
# stable API. That is deliberate and safe in one direction only: if the line cannot be
# found for a lockfile, this script FAILS (exit 2) asking for the parser to be updated. It
# must never degrade to a silent pass.
#
# SHIPPED-PATH GUARD (SMA-733). Some waivers in osv-scanner.toml are true only while no
# shipped package reaches the waived package. ci/osv/shipped_reachability.py walks
# ts/pnpm-lock.yaml and keeps that true for every advisory in its SHIPPED_FREE_WAIVERS
# table. This script runs it twice, `--self-test` first and then on the real files, and maps
# each exit code:
#
#   0          pass
#   3          the real run found a shipped path to a waived package (a finding, guard=1)
#   any other  infrastructure error (guard=2): 1 is a traceback, 2 is a guard refusal, 127
#              is a missing python3. A self-test that does not exit 0 is always guard=2.
#
# The findings block runs also when the guard failed, so a new, unrelated osv finding still
# prints. Exit precedence: 2 if the guard is 2 (or osv-scanner itself failed, above); else 1
# if osv found something or the guard is 1; else 0.
# Residual: nothing reds if someone deletes the guard calls below (SMA-733 spec section 4.5).
set -uo pipefail

cd "$(git rev-parse --show-toplevel)" || exit 2

LOCKFILES=(
  'ts/pnpm-lock.yaml'
  'py/uv.lock'
  'ci/workflow-credentials/uv.lock'
  # SMA-513 PR 2b — repo:helm-render resolves pyyaml through its own uv project, so its lockfile
  # is a fourth pip-ecosystem manifest. moon.yml's repo:osv inputs carry the same path.
  'ci/helm-render/uv.lock'
)

args=(scan source --config osv-scanner.toml --format json)
for lf in "${LOCKFILES[@]}"; do
  [ -f "$lf" ] || { echo "osv gate: lockfile '$lf' does not exist — update LOCKFILES in $0" >&2; exit 2; }
  args+=(--lockfile "$lf")
done

stderr_file="$(mktemp)"
json_file="$(mktemp)"
trap 'rm -f "$stderr_file" "$json_file"' EXIT

osv-scanner "${args[@]}" >"$json_file" 2>"$stderr_file"
rc=$?

# Hard errors: never let these read as "clean".
if [ "$rc" -ne 0 ] && [ "$rc" -ne 1 ]; then
  echo "osv gate: osv-scanner exited $rc (127=unresolvable path, 128=no package sources)." >&2
  cat "$stderr_file" >&2
  exit 2
fi

# --- Control: every lockfile must have contributed packages -------------------------------
for lf in "${LOCKFILES[@]}"; do
  line="$(grep -F "$lf file and found " "$stderr_file" | tail -1)"
  if [ -z "$line" ]; then
    echo "osv gate: no 'Scanned ... $lf file and found N packages' line in osv-scanner output." >&2
    echo "  The scanner did not report on this lockfile, or its stderr format changed." >&2
    echo "  Refusing to report a clean scan that may have covered nothing — update $0." >&2
    cat "$stderr_file" >&2
    exit 2
  fi
  # `packages\{0,1\}` — osv-scanner writes the count in the SINGULAR ("found 1 package") for a
  # one-package lockfile, which the plural-only expression parsed to the empty string, so
  # the zero-package branch below fired on a lockfile that had in fact been scanned
  # (SMA-593, measured on ci/workflow-credentials/uv.lock). Widening it does NOT weaken the
  # control: the assertion is `count is empty or 0`, and "found 0 packages" is itself
  # plural, so a genuinely empty lockfile still parses to 0 and still fires.
  count="$(printf '%s\n' "$line" | sed -n 's/.*found \([0-9][0-9]*\) packages\{0,1\}.*/\1/p')"
  if [ -z "$count" ] || [ "$count" -eq 0 ]; then
    echo "osv gate: '$lf' contributed 0 packages — it failed to parse, or the extractor regressed." >&2
    echo "  A zero-package lockfile makes 'no vulnerabilities' vacuous." >&2
    printf '  %s\n' "$line" >&2
    exit 2
  fi
  printf 'osv gate: %-22s %6s packages scanned\n' "$lf" "$count"
done

# --- Shipped-path guard (SMA-733) ---------------------------------------------------------
# No `-e` here, so each status is captured with `|| var=$?` and mapped explicitly. An
# unmapped status must never read as clean (see the header).
guard=0
selftest_rc=0
python3 ci/osv/shipped_reachability.py --self-test || selftest_rc=$?
guard_rc=0
python3 ci/osv/shipped_reachability.py --lockfile ts/pnpm-lock.yaml --config osv-scanner.toml || guard_rc=$?
if [ "$selftest_rc" -ne 0 ]; then
  echo "osv gate: the shipped-path guard's self-test exited $selftest_rc — the guard cannot be trusted." >&2
  guard=2
fi
case "$guard_rc" in
  0) ;;
  3) [ "$guard" -eq 2 ] || guard=1 ;;
  *)
    echo "osv gate: the shipped-path guard exited $guard_rc (1=traceback, 2=refusal, 127=no python3) — infrastructure error." >&2
    guard=2
    ;;
esac

# --- Findings -----------------------------------------------------------------------------
osv_found=0
if [ "$rc" -eq 1 ]; then
  osv_found=1
  echo "" >&2
  echo "osv gate: vulnerabilities found." >&2
  python3 - "$json_file" >&2 <<'PY'
import json, sys
with open(sys.argv[1]) as fh:
    data = json.load(fh)
for result in data.get('results', []):
    src = result.get('source', {}).get('path', '?')
    for pkg in result.get('packages', []):
        p = pkg.get('package', {})
        sev = max((g.get('max_severity') or '0') for g in pkg.get('groups', [{}])) or '?'
        ids = sorted({v.get('id') for v in pkg.get('vulnerabilities', []) if v.get('id')})
        aliases = sorted({a for v in pkg.get('vulnerabilities', []) for a in (v.get('aliases') or [])})
        fixed = sorted({
            ev['fixed']
            for v in pkg.get('vulnerabilities', [])
            for aff in v.get('affected', [])
            for rng in aff.get('ranges', [])
            for ev in rng.get('events', [])
            if 'fixed' in ev
        })
        print(f"  {p.get('ecosystem','?')}  {p.get('name','?')}@{p.get('version','?')}  (severity {sev})")
        print(f"      in      {src}")
        print(f"      ids     {', '.join(ids)}")
        if aliases:
            print(f"      aliases {', '.join(aliases)}")
        if fixed:
            print(f"      fixed   {', '.join(fixed)}")
PY
  echo "" >&2
  echo "  Fix by raising the resolved version (a pnpm-workspace.yaml override for a pinned" >&2
  echo "  npm transitive, or 'uv lock --upgrade-package <name>' for pip). Waive ONLY with a" >&2
  echo "  justified [[IgnoredVulns]] entry in osv-scanner.toml." >&2
fi

# --- Verdict --------------------------------------------------------------------------------
case "$guard" in 0) guard_word='pass' ;; 1) guard_word='shipped path found' ;; *) guard_word='infrastructure error' ;; esac
if [ "$osv_found" -eq 1 ]; then osv_word='vulnerabilities found'; else osv_word='no known vulnerabilities'; fi
echo "osv gate: summary: osv-scanner: $osv_word; shipped-path guard: $guard_word."
if [ "$guard" -eq 2 ]; then
  exit 2
fi
if [ "$osv_found" -eq 1 ] || [ "$guard" -eq 1 ]; then
  exit 1
fi
echo "osv gate: no known vulnerabilities in the npm or pip lockfiles."
