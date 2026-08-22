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

# Non-vacuity anchor (SMA-576 review finding 2): `run_check`'s own "checked == ${#SITES[@]}"
# guard is SELF-REFERENTIAL — deleting a row shrinks SITES and the expectation together, so it
# was measured to let a deleted row (e.g. napi-glue) through silently, printing
# "== all 17 … agree ==" and exiting 0 with the negative control and every other gate still
# green. This literal is the anchor: it ties SITES to a number recorded OUTSIDE this array, so
# a deleted (or accidentally duplicated) row now fails loudly instead. It can only ever
# FALSE-RED — forgetting to bump it after a deliberate SITES edit — never silently absorb a
# bypass, which is the correct failure direction for a gate whose whole job is not asserting
# vacuously. This is the documented fallback (a Moon-query-based comparison against the task's
# resolved inputs was judged impractical from inside a bash script with no dependency on the
# `moon` binary or a YAML parser): update it ONLY together with a deliberate SITES edit. The
# other half of this pin lives outside this file entirely, in
# ci_targets.py's SELF_TASK_EXPECTED_GLOBS["version-lockstep"] (part of repo:affected-smoke),
# which independently asserts moon.yml's own `inputs:` list — the paths SITES reads (15
# distinct, since two rows share py/packages/paigasus-kernel/pyproject.toml) plus rs/Cargo.toml
# (read by the cargo-wsdep kind by name, not by a SITES path) plus this script itself.
EXPECTED_SITE_COUNT=18

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
      # Every kernel-group member must be PRESENT in the lock, all at the same version.
      # Presence and uniformity are checked separately (SMA-576 review finding 4): comparing
      # only the DISTINCT set of versions found among names that DID match was measured to
      # pass vacuously when a name never appears at all — if three of four members vanished
      # from the lockfile, the one survivor's version still forms a set of size 1 and reads
      # as OK. A name absent from the lock is the repo being wrong (a stale `cargo update -w`,
      # or a workspace member dropped without relocking), same failure class as a version
      # mismatch, so it prints "" and reads as MISMATCH rather than exiting nonzero itself —
      # matching how a non-uniform version set already reports (empty string, not sys.exit).
      # An unreadable or undecodable lockfile is a DIFFERENT failure mode — infrastructure,
      # not drift — and exits 2 instead.
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import re, sys
p = sys.argv[1]
names = {"paigasus-kernel", "paigasus-py-bindings", "paigasus-node-bindings", "paigasus-wasm"}
try:
    text = open(p, encoding="utf-8").read()
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
present = set()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and n.group(1) in names:
        present.add(n.group(1))
        if v:
            found.add(v.group(1))
print(found.pop() if present == names and len(found) == 1 else "")
PY
      ;;
    uv-lock)
      # Same presence-plus-uniformity discipline as cargo-lock immediately above, and the
      # same SMA-576 review finding 4: a name missing from the lock entirely must not be
      # masked by the survivors' versions happening to agree.
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" <<'PY'
import re, sys
p = sys.argv[1]
names = {"paigasus-kernel", "paigasus-py-bindings"}
try:
    text = open(p, encoding="utf-8").read()
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
present = set()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and n.group(1) in names:
        present.add(n.group(1))
        if v:
            found.add(v.group(1))
print(found.pop() if present == names and len(found) == 1 else "")
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
  # SMA-576 review finding 2: check the literal anchor BEFORE anything else. This is what
  # catches a deleted SITES row that the loop-internal "checked == ${#SITES[@]}" guard below
  # cannot, because that guard is self-referential and shrinks in step with SITES.
  [ "${#SITES[@]}" -eq "$EXPECTED_SITE_COUNT" ] \
    || die_infra "SITES has ${#SITES[@]} entries, expected $EXPECTED_SITE_COUNT — this count must be updated deliberately alongside any SITES edit (see the comment above SITES; ci_targets.py's SELF_TASK_EXPECTED_GLOBS is the other half of this pin)"
  for group in "${!SOURCE_OF_TRUTH[@]}"; do
    # Explicit `|| return 2` rather than relying on errexit: when run_check is itself called
    # on the left of a `||` (as negative_control does), POSIX suspends errexit for run_check
    # AND everything it calls, so a read_version infra failure would otherwise be swallowed
    # into an empty string instead of propagating (SMA-576 review finding).
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")" || return 2
    [ -n "$expected" ] || die_infra "group '$group': source of truth has no version"
    printf 'group %s: source of truth = %s\n' "$group" "$expected"
  done
  local checked=0
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")" || return 2
    actual="$(read_version "$kind" "$target")" || return 2
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
  if [ "$ec" -eq 2 ]; then
    fail "negative control: run_check hit an infrastructure failure (exit 2) instead of
      reporting the drift. The scratch staging is incomplete or a site went unreadable —
      that is a broken control, not proof the gate can report red."
    return 1
  fi
  fail "negative control: a drifted site was ACCEPTED (run_check exited $ec, expected 1).
      The gate can no longer report red and is green exactly when it matters."
  return 1
}

write_site() { # $1 kind  $2 target  $3 version  -> prints 1 if it changed the file, else 0
  local kind="$1" target="$2" version="$3" abs="$REPO_ROOT/$2"
  case "$kind" in
    pyproject)
      # A page-wide (?m)^version\s*= substitution matches the FIRST such line anywhere in the
      # file, not [project]'s own — a `version =` key under an EARLIER TOML table (e.g.
      # [build-system] or a [tool.*] table that precedes [project]) would be rewritten
      # silently instead, since exactly one match is still found. Latent today only because
      # [project] happens to come first in every pyproject.toml this script writes. This is
      # the same unscoped-first-match class already hardened out of the packagejson arm below
      # (find_top_level_version_span) — scope this one to the [project] table specifically
      # (SMA-576 review finding 3).
      python3 - "$abs" "$version" <<'PY'
import re, sys

def find_project_version_match(s):
    """Return (re.Match, table_start) for the `version = "..."` line inside the [project]
    table specifically, or None. `table_start` is the offset of the table's body, so the
    match's spans can be re-anchored onto the whole-file string."""
    tm = re.search(r'(?m)^\[project\]\s*$', s)
    if tm is None:
        return None
    table_start = tm.end()
    nxt = re.search(r'(?m)^\[', s[table_start:])
    table_end = table_start + nxt.start() if nxt else len(s)
    vm = re.search(r'(?m)^(version\s*=\s*)"[^"]*"', s[table_start:table_end])
    if vm is None:
        return None
    return vm, table_start

p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
result = find_project_version_match(s)
if result is None:
    print("FATAL: no [project] version line", file=sys.stderr); raise SystemExit(2)
vm, table_start = result
start, end = table_start + vm.start(), table_start + vm.end()
new = s[:start] + f'{vm.group(1)}"{v}"' + s[end:]
open(p, "w", encoding="utf-8").write(new)
print(int(new != s))
PY
      ;;
    pyproject-dep)
      python3 - "$abs" "$version" <<'PY'
import re, sys
p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
new, n = re.subn(r'"paigasus-py-bindings(?:==[^"]*)?"', f'"paigasus-py-bindings=={v}"', s, count=1)
if n != 1:
    print("FATAL: no paigasus-py-bindings dependency", file=sys.stderr); raise SystemExit(2)
open(p, "w", encoding="utf-8").write(new)
print(int(new != s))
PY
      ;;
    packagejson)
      # Substitute the "version" field in place (like pyproject above) rather than round-
      # tripping through json.dumps: a full re-serialization reformats every array in the
      # file onto multiple lines (json.dumps has no compact-array mode), which would report
      # "wrote" on files whose version was already correct and pollute the diff with
      # unrelated Prettier-style churn (measured against this repo's committed package.json
      # files, SMA-576 review finding).
      #
      # A plain regex over the whole file matches the FIRST "version" key anywhere in the
      # text, not the object's own top-level one — a "version" key nested inside an earlier
      # object (e.g. an `engines` block) is rewritten instead, silently, since exactly one
      # match is still found. find_top_level_version_span walks the string tracking brace
      # depth and string-literal state (so braces/quotes inside string VALUES can't fool
      # it) and only returns the span of the value at depth 1 (SMA-576 review finding).
      python3 - "$abs" "$version" <<'PY'
import json, sys

def find_top_level_version_span(s):
    """Return (start, end) of the top-level "version" VALUE literal, or None."""
    depth = 0
    i = 0
    n = len(s)
    while i < n:
        c = s[i]
        if c == '"':
            j = i + 1
            while j < n:
                if s[j] == '\\':
                    j += 2
                    continue
                if s[j] == '"':
                    break
                j += 1
            key = s[i + 1:j]
            k = j + 1
            while k < n and s[k].isspace():
                k += 1
            if depth == 1 and key == "version" and k < n and s[k] == ':':
                k += 1
                while k < n and s[k].isspace():
                    k += 1
                if k >= n or s[k] != '"':
                    return None          # non-string version — refuse, do not guess
                v = k + 1
                while v < n:
                    if s[v] == '\\':
                        v += 2
                        continue
                    if s[v] == '"':
                        break
                    v += 1
                return (k, v + 1)
            i = j + 1
            continue
        if c in '{[':
            depth += 1
        elif c in '}]':
            depth -= 1
        i += 1
    return None

p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
try:
    json.loads(s)
except Exception as e:
    print(f"FATAL: malformed {p}: {e}", file=sys.stderr); raise SystemExit(2)
span = find_top_level_version_span(s)
if span is None:
    print(f"FATAL: no top-level string \"version\" field in {p}", file=sys.stderr)
    raise SystemExit(2)
start, end = span
new = s[:start] + f'"{v}"' + s[end:]
open(p, "w", encoding="utf-8").write(new)
print(int(new != s))
PY
      ;;
    *) printf '0' ;;   # release-plz- and regeneration-owned kinds are not written here
  esac
}

run_write() {
  local wrote=0 group kind target expected changed
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    case "$kind" in pyproject|pyproject-dep|packagejson) ;; *) continue ;; esac
    # Explicit `|| return 2` rather than relying on errexit: run_write may be invoked from
    # inside an `||` list, which suspends errexit for it and everything it calls, so a
    # failing capture would otherwise be swallowed into an empty string instead of
    # propagating as an infrastructure failure (same discipline as run_check, SMA-576).
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")" || return 2
    changed="$(write_site "$kind" "$target" "$expected")" || return 2
    wrote=$((wrote + changed))
  done

  # Regenerate the three derived sites (16-18). Each is owned by a tool, not by this script.
  ( cd "$REPO_ROOT/rs" && cargo update -w --offline >/dev/null 2>&1 ) \
    || ( cd "$REPO_ROOT/rs" && cargo update -w >/dev/null ) \
    || die_infra "cargo update -w failed (site 16)"
  ( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"
  # @napi-rs/cli is a devDependency of @paigasus/kernel, not of the ts workspace root
  # (pnpm-workspace.yaml's catalog comment: a file:-linked dep's devDeps aren't installed
  # at the consumer's node_modules root) — a bare `pnpm exec` from ts/ cannot find `napi`
  # and pnpm treats it as a recursive exec across every workspace package instead, failing
  # on the first one that lacks it. Scope it with --filter to the package that has it.
  ( cd "$REPO_ROOT/ts" && pnpm --filter @paigasus/kernel exec napi build --platform \
      --cwd "$REPO_ROOT/rs/crates/bindings/paigasus-node-bindings" >/dev/null ) \
    || die_infra "napi build failed (site 18)"

  if [ "$wrote" -gt 0 ]; then
    printf 'version-lockstep: wrote %d site(s)\n' "$wrote"
  else
    printf 'version-lockstep: already in lockstep\n'
  fi
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --check)             MODE=check; shift ;;
    --write)             MODE="write"; shift ;;
    --self-test)         MODE=selftest; shift ;;
    --negative-control)  MODE=negctl; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_self_tests ;;
  check)    run_check ;;
  write)    run_write ;;
  negctl)   negative_control ;;
esac
