#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:version-lockstep — asserts every version-carrying site in a lockstep family agrees
# with that family's source-of-truth Cargo crate (SMA-576, spec §4).
#
# Exit codes: 0 pass | 1 assertion failed (the repo is wrong) | 2 infrastructure failed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

die_infra() { printf 'INFRA: %s\n' "$*" >&2; exit 2; }
fail()      { printf 'FAIL: %s\n' "$*" >&2; }

# The ONE maintained fact in this script: the family membership table.
# Format: <group>|<kind>|<path>
# kind ∈ cargo-package | cargo-wsdep | pyproject | pyproject-dep | packagejson | cargo-lock | uv-lock | napi-glue
SITES=(
  "kernel|cargo-package|rs/crates/libs/paigasus-kernel/Cargo.toml"
  "kernel|cargo-package|rs/crates/bindings/paigasus-py-bindings/Cargo.toml"
  "kernel|cargo-package|rs/crates/bindings/paigasus-node-bindings/Cargo.toml"
  "kernel|cargo-package|rs/crates/bindings/paigasus-wasm/Cargo.toml"
  "proto|cargo-package|rs/crates/libs/paigasus-proto/Cargo.toml"
  "proto|cargo-package|rs/crates/libs/paigasus-proto-derive/Cargo.toml"
  "kernel|cargo-wsdep|paigasus-kernel"
  "proto|cargo-wsdep|paigasus-proto"
  "proto|cargo-wsdep|paigasus-proto-derive"
  "kernel|pyproject|rs/crates/bindings/paigasus-py-bindings/pyproject.toml"
  "kernel|pyproject|py/packages/paigasus-kernel/pyproject.toml"
  "proto|pyproject|py/packages/paigasus-proto/pyproject.toml"
  "kernel|packagejson|rs/crates/bindings/paigasus-node-bindings/package.json"
  "kernel|packagejson|rs/crates/bindings/paigasus-wasm/package.json"
  "kernel|pyproject-dep|py/packages/paigasus-kernel/pyproject.toml"
  "kernel|cargo-lock|rs/Cargo.lock"
  "kernel|uv-lock|py/uv.lock"
  "kernel|napi-glue|rs/crates/bindings/paigasus-node-bindings/index.js"
)

# Source of truth per group.
declare -A SOURCE_OF_TRUTH=(
  [kernel]="rs/crates/libs/paigasus-kernel/Cargo.toml"
  [proto]="rs/crates/libs/paigasus-proto/Cargo.toml"
)

SELF_TESTS_RAN=0
SELF_TEST_COUNT=1   # site_verdict

site_verdict() { # $1 expected  $2 actual
  if [ -n "$2" ] && [ "$1" = "$2" ]; then printf 'OK'; else printf 'MISMATCH'; fi
}

read_version() { # $1 kind  $2 path-or-name
  local kind="$1" target="$2" abs="$REPO_ROOT/$2"
  case "$kind" in
    cargo-package)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import sys, tomllib
p = sys.argv[1]
try:
    v = tomllib.load(open(p, "rb"))["package"]["version"]
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
print(v)
PY
      ;;
    cargo-wsdep)
      [ -r "$REPO_ROOT/rs/Cargo.toml" ] || die_infra "cannot read rs/Cargo.toml"
      python3 - "$REPO_ROOT/rs/Cargo.toml" "$target" <<'PY'
import sys, tomllib
p = sys.argv[1]
try:
    deps = tomllib.load(open(p, "rb"))["workspace"]["dependencies"]
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
d = deps.get(sys.argv[2])
print(d.get("version", "") if isinstance(d, dict) else "")
PY
      ;;
    pyproject)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import sys, tomllib
p = sys.argv[1]
try:
    v = tomllib.load(open(p, "rb"))["project"]["version"]
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
print(v)
PY
      ;;
    pyproject-dep)
      # The pin on paigasus-py-bindings. An UNPINNED dep prints "" and so reads as MISMATCH —
      # which is the point: uv strips [tool.uv.sources] from the built wheel, so an unpinned
      # wrapper would float against any bindings version once published (spec §4). A malformed
      # or unparsable pyproject.toml is a DIFFERENT failure mode — infrastructure, not drift —
      # and exits 2 instead.
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import re, sys, tomllib
p = sys.argv[1]
try:
    deps = tomllib.load(open(p, "rb"))["project"].get("dependencies", [])
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
for d in deps:
    m = re.fullmatch(r"paigasus-py-bindings==([0-9][^,;\s]*)", d.strip())
    if m:
        print(m.group(1)); break
else:
    print("")
PY
      ;;
    packagejson)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import json, sys
p = sys.argv[1]
try:
    v = json.load(open(p))["version"]
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
print(v)
PY
      ;;
    cargo-lock)
      # Every kernel-group member must appear in the lock at the group version. Prints the
      # DISTINCT set; anything but a single value reads as MISMATCH against the expected —
      # that is the repo being wrong, not infrastructure failing. An unreadable or
      # undecodable lockfile is infrastructure failing, and exits 2 instead.
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import re, sys
p = sys.argv[1]
names = {"paigasus-kernel", "paigasus-py-bindings", "paigasus-node-bindings", "paigasus-wasm"}
try:
    text = open(p, encoding="utf-8").read()
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and v and n.group(1) in names:
        found.add(v.group(1))
print(found.pop() if len(found) == 1 else "")
PY
      ;;
    uv-lock)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import re, sys
p = sys.argv[1]
names = {"paigasus-kernel", "paigasus-py-bindings"}
try:
    text = open(p, encoding="utf-8").read()
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and v and n.group(1) in names:
        found.add(v.group(1))
print(found.pop() if len(found) == 1 else "")
PY
      ;;
    napi-glue)
      # napi regenerates 26 `bindingPackageVersion !== '<v>'` guards from package.json.
      # A non-uniform set prints "" and reads as MISMATCH; an unreadable or undecodable
      # file exits 2.
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import re, sys
p = sys.argv[1]
try:
    text = open(p, encoding="utf-8").read()
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
vs = set(re.findall(r"bindingPackageVersion !== '([^']+)'", text))
print(vs.pop() if len(vs) == 1 else "")
PY
      ;;
    *) die_infra "unknown site kind: $kind" ;;
  esac
}

site_verdict_self_test() {
  local got
  got="$(site_verdict "0.1.0" "0.1.0")"
  [ "$got" = "OK" ] || { fail "self-test: equal versions should be OK, got '$got'"; return 1; }
  got="$(site_verdict "0.1.0" "0.0.0")"
  [ "$got" = "MISMATCH" ] || { fail "self-test: differing versions should be MISMATCH, got '$got'"; return 1; }
  got="$(site_verdict "0.1.0" "")"
  [ "$got" = "MISMATCH" ] || { fail "self-test: an absent version should be MISMATCH, got '$got'"; return 1; }
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

run_self_tests() {
  SELF_TESTS_RAN=0
  site_verdict_self_test
  [ "$SELF_TESTS_RAN" -eq "$SELF_TEST_COUNT" ] \
    || die_infra "self-tests ran $SELF_TESTS_RAN, expected $SELF_TEST_COUNT"
  printf '== version-lockstep self-tests passed (%d tables) ==\n' "$SELF_TESTS_RAN"
}

run_check() {
  local rc=0 group kind target expected actual verdict
  for group in "${!SOURCE_OF_TRUTH[@]}"; do
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")"
    [ -n "$expected" ] || die_infra "group '$group': source of truth has no version"
    printf 'group %s: source of truth = %s\n' "$group" "$expected"
  done
  local checked=0
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")"
    actual="$(read_version "$kind" "$target")"
    verdict="$(site_verdict "$expected" "$actual")"
    checked=$((checked + 1))
    if [ "$verdict" != OK ]; then
      fail "[$group] $kind $target: expected '$expected', found '${actual:-<absent or non-uniform>}'"
      rc=1
    fi
  done
  # Non-vacuity: the loop must have covered every declared site.
  [ "$checked" -eq "${#SITES[@]}" ] \
    || die_infra "checked $checked sites but ${#SITES[@]} are declared"
  if [ "$rc" -eq 0 ]; then
    printf '== all %d version-lockstep sites agree ==\n' "$checked"
  fi
  return "$rc"
}

# Copy the tree's version-carrying files into a scratch dir, drift ONE site, and assert the
# checker reports red. Driving the real run_check (not a reimplementation) is what makes this
# a control rather than a second, differently-wrong checker.
negative_control() {
  local tmp
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  # Stage exactly the files the SITES table names, plus rs/Cargo.toml (which the cargo-wsdep
  # kind reads by name rather than by path). Deriving the list from SITES rather than from a
  # hand-written glob keeps the control honest when a site is added: a new site is staged
  # automatically, so the control cannot quietly stop covering it.
  local entry kind target
  {
    printf 'rs/Cargo.toml\n'
    for entry in "${SITES[@]}"; do
      IFS='|' read -r _ kind target <<<"$entry"
      [ "$kind" = cargo-wsdep ] || printf '%s\n' "$target"
    done
  } | sort -u | ( cd "$REPO_ROOT" && tar -cf - -T - ) | ( cd "$tmp" && tar -xf - ) \
    || die_infra "cannot stage a scratch copy of the version-carrying files"

  # Drift site 13 (@paigasus/node-bindings) to a version no group member carries.
  python3 - "$tmp/rs/crates/bindings/paigasus-node-bindings/package.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["version"] = "99.99.99"
json.dump(d, open(p, "w"), indent=2)
PY

  local ec=0
  REPO_ROOT="$tmp" run_check >/dev/null 2>&1 || ec=$?
  if [ "$ec" -eq 1 ]; then
    printf '== negative control: version-lockstep reported red as expected ==\n'
    return 0
  fi
  fail "negative control: a drifted site was ACCEPTED (run_check exited $ec, expected 1).
      The gate can no longer report red and is green exactly when it matters."
  return 1
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --check)             MODE=check; shift ;;
    --self-test)         MODE=selftest; shift ;;
    --negative-control)  MODE=negctl; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_self_tests ;;
  check)    run_check ;;
  negctl)   negative_control ;;
esac
