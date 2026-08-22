#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:version-lockstep — asserts every version-carrying site in a lockstep family agrees
# with that family's source-of-truth Cargo crate (SMA-576, spec §4).
#
# Exit codes: 0 pass | 1 assertion failed (the repo is wrong) | 2 infrastructure failed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SELF_SRC="${BASH_SOURCE[0]}"

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
      python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$abs"
      ;;
    cargo-wsdep)
      python3 -c '
import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))["workspace"]["dependencies"].get(sys.argv[2])
print(d.get("version", "") if isinstance(d, dict) else "")
' "$REPO_ROOT/rs/Cargo.toml" "$target"
      ;;
    pyproject)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["project"]["version"])' "$abs"
      ;;
    pyproject-dep)
      # The pin on paigasus-py-bindings. An UNPINNED dep prints "" and so reads as MISMATCH —
      # which is the point: uv strips [tool.uv.sources] from the built wheel, so an unpinned
      # wrapper would float against any bindings version once published (spec §4).
      python3 -c '
import re, sys, tomllib
deps = tomllib.load(open(sys.argv[1], "rb"))["project"].get("dependencies", [])
for d in deps:
    m = re.fullmatch(r"paigasus-py-bindings==([0-9][^,;\s]*)", d.strip())
    if m:
        print(m.group(1)); break
else:
    print("")
' "$abs"
      ;;
    packagejson)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$abs"
      ;;
    cargo-lock)
      # Every kernel-group member must appear in the lock at the group version. Prints the
      # DISTINCT set; anything but a single value reads as MISMATCH against the expected.
      python3 -c '
import re, sys
names = {"paigasus-kernel", "paigasus-py-bindings", "paigasus-node-bindings", "paigasus-wasm"}
text = open(sys.argv[1], encoding="utf-8").read()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and v and n.group(1) in names:
        found.add(v.group(1))
print(found.pop() if len(found) == 1 else "")
' "$abs"
      ;;
    uv-lock)
      python3 -c '
import re, sys
names = {"paigasus-kernel", "paigasus-py-bindings"}
text = open(sys.argv[1], encoding="utf-8").read()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and v and n.group(1) in names:
        found.add(v.group(1))
print(found.pop() if len(found) == 1 else "")
' "$abs"
      ;;
    napi-glue)
      # napi regenerates 26 `bindingPackageVersion !== '<v>'` guards from package.json.
      # A non-uniform set prints "" and reads as MISMATCH.
      python3 - "$abs" <<'PY'
import re, sys
vs = set(re.findall(r"bindingPackageVersion !== '([^']+)'", open(sys.argv[1], encoding="utf-8").read()))
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

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --check)     MODE=check; shift ;;
    --self-test) MODE=selftest; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_self_tests ;;
  check)    run_check ;;
esac
