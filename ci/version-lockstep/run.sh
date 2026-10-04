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
  "proto|cargo-lock|rs/Cargo.lock"
  "proto|uv-lock|py/uv.lock"
)

# Non-vacuity anchor (SMA-576 review finding 2): `run_check`'s own "checked == ${#SITES[@]}"
# guard is SELF-REFERENTIAL — deleting a row shrinks SITES and the expectation together, so it
# was measured to let a deleted row (e.g. napi-glue) through silently, printing
# "== all 19 … agree ==" and exiting 0 with the negative control and every other gate still
# green. This literal is the anchor: it ties SITES to a number recorded OUTSIDE this array, so
# a deleted (or accidentally duplicated) row now fails loudly instead. It can only ever
# FALSE-RED — forgetting to bump it after a deliberate SITES edit — never silently absorb a
# bypass, which is the correct failure direction for a gate whose whole job is not asserting
# vacuously. This is the documented fallback (a Moon-query-based comparison against the task's
# resolved inputs was judged impractical from inside a bash script with no dependency on the
# `moon` binary or a YAML parser): update it ONLY together with a deliberate SITES edit. The
# other half of this pin lives outside this file entirely, in
# ci_targets.py's SELF_TASK_EXPECTED_GLOBS["version-lockstep"] (part of repo:affected-smoke),
# which independently asserts moon.yml's own `inputs:` list — the paths SITES reads (14
# distinct: py/packages/paigasus-kernel/pyproject.toml, rs/Cargo.lock, and py/uv.lock are each
# read by two rows) plus rs/Cargo.toml (read by the cargo-wsdep kind by name, not by a SITES
# path) plus this script itself, plus py/pyproject.toml and the glob py/packages/*/pyproject.toml
# (read by uv_static_metadata_check, SMA-684) — 18 entries, matching moon.yml's inputs: list.
EXPECTED_SITE_COUNT=20

# Source of truth per group.
declare -A SOURCE_OF_TRUTH=(
  [kernel]="rs/crates/libs/paigasus-kernel/Cargo.toml"
  [proto]="rs/crates/libs/paigasus-proto/Cargo.toml"
)

# Lock-file membership, keyed <group>:<kind> — NOT by group alone, BECAUSE THE TWO KINDS
# SPAN TWO NAMESPACES. cargo-lock names are Cargo crate names; uv-lock names are Python
# distribution names. paigasus-node-bindings and paigasus-wasm are npm artifacts that
# appear nowhere in py/uv.lock, and paigasus-proto-derive is a proc-macro crate with no
# Python distribution at all. A single per-group set would demand it in py/uv.lock — a
# permanent false red — or silently weaken the check.
declare -A LOCK_MEMBERS=(
  [kernel:cargo-lock]="paigasus-kernel paigasus-py-bindings paigasus-node-bindings paigasus-wasm"
  [kernel:uv-lock]="paigasus-kernel paigasus-py-bindings"
  [proto:cargo-lock]="paigasus-proto paigasus-proto-derive"
  [proto:uv-lock]="paigasus-proto"
)

SELF_TESTS_RAN=0
SELF_TEST_COUNT=6   # site_verdict, lock_reader, cargo_package_writer, stamp_sites, napi_glue_writer, uv_static_metadata

site_verdict() { # $1 expected  $2 actual
  if [ -n "$2" ] && [ "$1" = "$2" ]; then printf 'OK'; else printf 'MISMATCH'; fi
}

read_version() { # $1 kind  $2 path-or-name  $3 group (required for lock kinds)
  local kind="$1" target="$2" group="${3:-}" abs="$REPO_ROOT/$2"
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
      [ -n "$group" ] || die_infra "cargo-lock site for '$target' was read without a group"
      local members="${LOCK_MEMBERS[$group:cargo-lock]:-}"
      [ -n "$members" ] || die_infra "no LOCK_MEMBERS entry for '$group:cargo-lock'"
      python3 - "$abs" "$members" <<'PY'
import re, sys
p, names = sys.argv[1], set(sys.argv[2].split())
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
      [ -n "$group" ] || die_infra "uv-lock site for '$target' was read without a group"
      local members="${LOCK_MEMBERS[$group:uv-lock]:-}"
      [ -n "$members" ] || die_infra "no LOCK_MEMBERS entry for '$group:uv-lock'"
      python3 - "$abs" "$members" <<'PY'
import re, sys
p, names = sys.argv[1], set(sys.argv[2].split())
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
      # The 27 `bindingPackageVersion !== '<v>'` guards (26 native, 1 WASI). --write's text writer
      # (write_site's napi-glue arm, SMA-684) writes them, and this arm reads them back. A
      # non-uniform set prints "" and reads as MISMATCH; an unreadable or undecodable file exits 2.
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

# L2 closure for the lock readers: EXPECTED_SITE_COUNT cannot see a WRONG name set, and the
# negative control drifts a packagejson site, so before this table neither lock arm was
# exercised at all. Dropping paigasus-proto-derive from [proto:cargo-lock] would have been
# a silent false-green on the very change that introduced the table.
lock_reader_self_test() {
  local tmp got
  tmp="$(mktemp -d)"
  mkdir -p "$tmp/rs" "$tmp/py"

  # All members present at a uniform version -> that version.
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n\n[[package]]\nname = "paigasus-proto-derive"\nversion = "0.1.0"\n' \
    >"$tmp/rs/Cargo.lock"
  got="$(REPO_ROOT="$tmp" read_version cargo-lock rs/Cargo.lock proto)"
  [ "$got" = "0.1.0" ] || { fail "self-test: uniform proto cargo-lock should read 0.1.0, got '$got'"; rm -rf "$tmp"; return 1; }

  # A MEMBER MISSING must read as "" (MISMATCH), not as the survivor's version.
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n' >"$tmp/rs/Cargo.lock"
  got="$(REPO_ROOT="$tmp" read_version cargo-lock rs/Cargo.lock proto)"
  [ -z "$got" ] || { fail "self-test: a missing cargo-lock member must read '', got '$got'"; rm -rf "$tmp"; return 1; }

  # Non-uniform versions must read "".
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n\n[[package]]\nname = "paigasus-proto-derive"\nversion = "0.2.0"\n' \
    >"$tmp/rs/Cargo.lock"
  got="$(REPO_ROOT="$tmp" read_version cargo-lock rs/Cargo.lock proto)"
  [ -z "$got" ] || { fail "self-test: a non-uniform cargo-lock must read '', got '$got'"; rm -rf "$tmp"; return 1; }

  # The uv-lock arm reads its OWN namespace: proto's uv membership is one name.
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n' >"$tmp/py/uv.lock"
  got="$(REPO_ROOT="$tmp" read_version uv-lock py/uv.lock proto)"
  [ "$got" = "0.1.0" ] || { fail "self-test: proto uv-lock should read 0.1.0, got '$got'"; rm -rf "$tmp"; return 1; }

  rm -rf "$tmp"
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

# SMA-685: fixture table for the cargo-package writer. The fixtures deliberately vary the
# layout: spacing, comments, table order, CRLF, and no trailing newline. A writer tested on
# one layout only passes on the layout its author assumed.
cargo_package_writer_self_test() {
  local tmp rc got before after
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  _cpw() { # $1 fixture file (relative to $tmp)  $2 head version -> sets rc and got
    rc=0
    got="$(REPO_ROOT="$tmp" write_site cargo-package "$1" "$2" 2>/dev/null)" || rc=$?
  }
  _cpw_expect() { # $1 fixture  $2 expected file content (printf format)
    local want
    want="$(printf "$2")"
    [ "$(cat "$tmp/$1")" = "$want" ] \
      || { fail "self-test: cargo-package writer produced the wrong text for $1"; return 1; }
  }

  # F1: plain layout; rust-version, a dependency version and an inline table stay untouched.
  printf '[package]\nname = "a"\nversion = "0.1.0"\nrust-version = "1.95"\n\n[dependencies]\nfoo = { version = "0.1.0" }\n\n[dependencies.bar]\nversion = "0.1.0"\n' >"$tmp/f1.toml"
  _cpw f1.toml 0.2.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: F1 rc=$rc got='$got', expected rc 0 and 1"; return 1; }
  _cpw_expect f1.toml '[package]\nname = "a"\nversion = "0.2.0"\nrust-version = "1.95"\n\n[dependencies]\nfoo = { version = "0.1.0" }\n\n[dependencies.bar]\nversion = "0.1.0"' || return 1

  # F2: idempotent — same version prints 0, changes no byte and keeps the mtime.
  touch -t 200001010000 "$tmp/f1.toml"
  before="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/f1.toml")"
  _cpw f1.toml 0.2.0
  after="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/f1.toml")"
  [ "$rc" -eq 0 ] && [ "$got" = 0 ] && [ "$before" = "$after" ] \
    || { fail "self-test: F2 idempotence rc=$rc got='$got' mtime $before -> $after"; return 1; }

  # F3: spacing variants and a trailing comment on the version line.
  printf '[package]\nname="b"\nversion   =  "0.1.0"   # the floor\n' >"$tmp/f3.toml"
  _cpw f3.toml 0.1.1
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: F3 rc=$rc got='$got'"; return 1; }
  _cpw_expect f3.toml '[package]\nname="b"\nversion   =  "0.1.1"   # the floor' || return 1

  # F4: a commented header and comment lines between [package] and version.
  printf '# top\n[package] # the crate\nname = "c"\n# The floor. version = "9.9.9" in a comment.\n# more\nversion = "0.1.0"\n' >"$tmp/f4.toml"
  _cpw f4.toml 0.3.0
  [ "$rc" -eq 0 ] || { fail "self-test: F4 rc=$rc"; return 1; }
  _cpw_expect f4.toml '# top\n[package] # the crate\nname = "c"\n# The floor. version = "9.9.9" in a comment.\n# more\nversion = "0.3.0"' || return 1

  # F5: CRLF line endings are kept.
  printf '[package]\r\nname = "d"\r\nversion = "0.1.0"\r\n' >"$tmp/f5.toml"
  _cpw f5.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F5 rc=$rc"; return 1; }
  [ "$(od -An -c "$tmp/f5.toml" | tr -d ' \n')" = "$(printf '[package]\r\nname = "d"\r\nversion = "0.2.0"\r\n' | od -An -c | tr -d ' \n')" ] \
    || { fail "self-test: F5 CRLF was not kept"; return 1; }

  # F6: [package] is not the first table, and [package.metadata.x] has its own version key.
  printf '[lib]\npath = "src/lib.rs"\n\n[package]\nname = "e"\nversion = "0.1.0"\n\n[package.metadata.x]\nversion = "7.7.7"\n' >"$tmp/f6.toml"
  _cpw f6.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F6 rc=$rc"; return 1; }
  _cpw_expect f6.toml '[lib]\npath = "src/lib.rs"\n\n[package]\nname = "e"\nversion = "0.2.0"\n\n[package.metadata.x]\nversion = "7.7.7"' || return 1

  # F7: a version key in a table BEFORE [package] stays untouched.
  printf '[dependencies.foo]\nversion = "0.1.0"\n\n[package]\nname = "f"\nversion = "0.1.0"\n' >"$tmp/f7.toml"
  _cpw f7.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F7 rc=$rc"; return 1; }
  _cpw_expect f7.toml '[dependencies.foo]\nversion = "0.1.0"\n\n[package]\nname = "f"\nversion = "0.2.0"' || return 1

  # F8-F9, F11: shapes the writer must refuse with rc 2, leaving the file unchanged.
  local f
  printf '[package]\nname = "g"\nversion.workspace = true\n' >"$tmp/f8a.toml"
  printf '[lib]\npath = "x"\n' >"$tmp/f8b.toml"
  printf '[package]\nname = "g"\n' >"$tmp/f8c.toml"
  # F9: a literal duplicate `version` key at the top level. This is invalid TOML, so
  # tomllib.loads refuses it before the writer's OWN `len(keys) != 1` guard ever runs. F9b
  # below is the guard's real fixture (SMA-685 review F5).
  printf '[package]\nname = "g"\nversion = "0.1.0"\nversion = "0.1.0"\n' >"$tmp/f9.toml"
  # F9b: VALID TOML with one real `version` key, but a multi-line string body that also
  # contains a line starting with "version = ". The `[package]` body is scanned as text, not
  # parsed structurally, so this line matches the writer's key-count regex too — two matches,
  # not one — and the guard must refuse (SMA-685 review F5). Proven by mutation below.
  printf '[package]\nname = "g"\ndescription = """\nversion = "5.5.5"\n"""\nversion = "0.1.0"\n' >"$tmp/f9b.toml"
  printf '[package]\nname = "g"\nversion = "0.1.0-rc.1"\n' >"$tmp/f11.toml"
  for f in f8a f8b f8c f9 f9b f11; do
    before="$(cat "$tmp/$f.toml")"
    _cpw "$f.toml" 0.2.0
    [ "$rc" -eq 2 ] || { fail "self-test: $f must be refused with rc 2, got rc=$rc"; return 1; }
    [ "$(cat "$tmp/$f.toml")" = "$before" ] || { fail "self-test: $f was changed although refused"; return 1; }
  done

  # F10: refuses to lower a version. This calls write_site directly, so it sees write_site's
  # OWN exit code, rc 3 (SMA-685 review F1) — not stamp_sites' mapped rc 1. See
  # stamp_sites_self_test for the rc 3 -> rc 1 mapping, exercised at the stamp_sites level.
  printf '[package]\nname = "h"\nversion = "0.2.0"\n' >"$tmp/f10.toml"
  _cpw f10.toml 0.1.0
  [ "$rc" -eq 3 ] || { fail "self-test: F10 lowering must be rc 3, got rc=$rc"; return 1; }
  _cpw_expect f10.toml '[package]\nname = "h"\nversion = "0.2.0"' || return 1

  # F12: a multi-line description with a line that starts with "[" — never a wrong edit.
  printf '[package]\nname = "i"\ndescription = """\n[not a table]\nversion = "5.5.5"\n"""\nversion = "0.1.0"\n' >"$tmp/f12.toml"
  _cpw f12.toml 0.2.0
  if [ "$rc" -eq 0 ]; then
    _cpw_expect f12.toml '[package]\nname = "i"\ndescription = """\n[not a table]\nversion = "5.5.5"\n"""\nversion = "0.2.0"' || return 1
  else
    [ "$rc" -eq 2 ] || { fail "self-test: F12 must be a correct edit or rc 2, got rc=$rc"; return 1; }
  fi

  # F13: no trailing newline — none is added.
  printf '[package]\nname = "j"\nversion = "0.1.0"' >"$tmp/f13.toml"
  _cpw f13.toml 0.2.0
  [ "$rc" -eq 0 ] || { fail "self-test: F13 rc=$rc"; return 1; }
  [ "$(od -An -c "$tmp/f13.toml" | tr -d ' \n')" = "$(printf '[package]\nname = "j"\nversion = "0.2.0"' | od -An -c | tr -d ' \n')" ] \
    || { fail "self-test: F13 changed the file end"; return 1; }

  # SMA-685 review F7: publish.workspace = true is unresolved inheritance, not a knowable
  # boolean. cargo_publish_false must fail closed (rc 2), not guess "publishable".
  printf '[package]\nname = "k"\npublish.workspace = true\n' >"$tmp/f14.toml"
  rc=0
  got="$(REPO_ROOT="$tmp" cargo_publish_false f14.toml 2>/dev/null)" || rc=$?
  [ "$rc" -eq 2 ] \
    || { fail "self-test: publish.workspace = true must be refused with rc 2, got rc=$rc got='$got'"; return 1; }

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

# SMA-685 review F6+F15: force a Cargo [package] version line to an exact literal, independent
# of write_site (the function under test elsewhere). Used to plant sentinel and drift versions.
# Prints nothing on success. On a known failure, such as no version line found, it prints one
# FATAL line and returns 2. On an unexpected failure, such as a missing file, it may instead
# raise an uncaught python error and return non-zero without that FATAL line. Every caller
# treats any non-zero return as an infrastructure failure, and removes its own scratch
# directory before it dies.
_force_cargo_version() { # $1 file  $2 version
  local file="$1" version="$2" rc=0
  python3 - "$file" "$version" <<'PY' || rc=$?
import re, sys
p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
s, n = re.subn(r'(?m)^version = "[^"]*"$', f'version = "{v}"', s, count=1)
if n != 1:
    print(f"FATAL: no version line in {p}", file=sys.stderr)
    raise SystemExit(2)
open(p, "w", encoding="utf-8").write(s)
PY
  return "$rc"
}

# SMA-685: run the PRODUCTION loop (stamp_sites) on a staged copy of the real tree. The fixture
# table above tests write_site alone and cannot see a kind dropped from stamp_sites' filter; this
# table can, and it runs the writer on the real manifest shapes.
stamp_sites_self_test() {
  local tmp rc=0 entry group kind target got derive_before derive_after
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  stage_pristine_tree "$tmp"
  derive_before="$(REPO_ROOT="$tmp" read_version cargo-package rs/crates/libs/paigasus-proto-derive/Cargo.toml)" || return 2

  # SMA-685 review F4: the sentinels below must not collide with the real tree's own data, or a
  # writer bug that fails to move a site could coincidentally match the sentinel already there
  # and the assertions below would pass for the wrong reason. Check this BEFORE stamping.
  #
  # SMA-685 review F4 (round 2): this guard must read every site the two readback loops below
  # assert. The old two-loop version read only the cargo-package sites. A kernel pyproject,
  # pyproject-dep or packagejson site could already hold a sentinel value. So could the one
  # proto pyproject site. Either case would make part of the readback assertion vacuous.
  #
  # This is now ONE loop over every kernel and proto site whose kind the readback loops check:
  # cargo-package, pyproject, pyproject-dep, and packagejson. It reads each file once. It fails
  # closed (rc 2) if a group has no such sites, or if any read is empty.
  local kv="" pv="" kcount=0 pcount=0
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    case "$kind" in cargo-package|pyproject|pyproject-dep|packagejson|napi-glue) ;; *) continue ;; esac
    got="$(REPO_ROOT="$tmp" read_version "$kind" "$target" "$group")" || return 2
    [ -n "$got" ] \
      || { rm -rf "$tmp"; die_infra "self-test: $kind $target read back empty while validating the sentinels"; }
    case "$group" in
      kernel) kv="$kv $got"; kcount=$((kcount + 1)) ;;
      proto)  pv="$pv $got"; pcount=$((pcount + 1)) ;;
    esac
  done
  [ "$kcount" -gt 0 ] \
    || { rm -rf "$tmp"; die_infra "self-test: the kernel group has no sites to validate the sentinel against"; }
  [ "$pcount" -gt 0 ] \
    || { rm -rf "$tmp"; die_infra "self-test: the proto group has no sites to validate the sentinel against"; }
  python3 - "9.9.9" "8.8.8" "$derive_before" "$kv" "$pv" <<'PY' \
    || { rm -rf "$tmp"; die_infra "self-test: the sentinel versions are not valid; see stderr above"; }
import sys

def tup(v):
    return tuple(int(x) for x in v.split("."))

kernel_sentinel, proto_sentinel, derive_before = sys.argv[1], sys.argv[2], sys.argv[3]
kernel_versions, proto_versions = sys.argv[4].split(), sys.argv[5].split()

if kernel_sentinel == proto_sentinel:
    print("FATAL: the kernel and proto sentinels must differ", file=sys.stderr)
    sys.exit(1)
if kernel_sentinel == derive_before or proto_sentinel == derive_before:
    print("FATAL: a sentinel must differ from the real paigasus-proto-derive version", file=sys.stderr)
    sys.exit(1)
for v in kernel_versions:
    if tup(kernel_sentinel) <= tup(v):
        print(f"FATAL: the kernel sentinel {kernel_sentinel} is not higher than site version {v}", file=sys.stderr)
        sys.exit(1)
for v in proto_versions:
    if tup(proto_sentinel) <= tup(v):
        print(f"FATAL: the proto sentinel {proto_sentinel} is not higher than site version {v}", file=sys.stderr)
        sys.exit(1)
PY

  # Move the kernel head to a sentinel, independently of the writer under test.
  _force_cargo_version "$tmp/rs/crates/libs/paigasus-kernel/Cargo.toml" "9.9.9" \
    || { rm -rf "$tmp"; die_infra "cannot force the kernel head to its sentinel version"; }
  # Move the proto head to a SECOND, different sentinel.
  # paigasus-proto-derive is publishable, not publish = false, and it is a non-head cargo-package
  # site. In the real tree, it starts at the same version as the proto head.
  # So this test needs a second, different sentinel. Without it, a mutation could delete the
  # publish = false filter, and derive_before could then equal derive_after by coincidence.
  # This table would then miss the mutation.
  _force_cargo_version "$tmp/rs/crates/libs/paigasus-proto/Cargo.toml" "8.8.8" \
    || { rm -rf "$tmp"; die_infra "cannot force the proto head to its sentinel version"; }
  REPO_ROOT="$tmp" stamp_sites >/dev/null || rc=$?
  [ "$rc" -eq 0 ] || { fail "self-test: stamp_sites on the staged tree returned $rc"; return 1; }
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    [ "$group" = kernel ] || continue
    case "$kind" in cargo-package|pyproject|pyproject-dep|packagejson|napi-glue) ;; *) continue ;; esac
    got="$(REPO_ROOT="$tmp" read_version "$kind" "$target" "$group")" || return 2
    [ "$got" = "9.9.9" ] || { fail "self-test: stamp_sites left $kind $target at '$got', expected 9.9.9"; return 1; }
  done
  # SMA-684: read_version's napi-glue arm reads the G1 literals only. Count both literal forms in
  # the stamped file, so a writer that moved G1 and left G2 behind cannot pass this table.
  got="$(python3 - "$tmp/rs/crates/bindings/paigasus-node-bindings/index.js" <<'PY'
import sys
with open(sys.argv[1], encoding="utf-8") as f:
    s = f.read()
print(s.count("bindingPackageVersion !== '9.9.9'"), s.count("expected 9.9.9 but got"))
PY
)" || return 2
  [ "${got% *}" -gt 0 ] && [ "${got% *}" = "${got#* }" ] \
    || { fail "self-test: stamp_sites left the napi glue G1/G2 counts at '$got', expected two equal non-zero counts at 9.9.9"; return 1; }

  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    [ "$group" = proto ] || continue
    case "$kind" in pyproject|pyproject-dep|packagejson) ;; *) continue ;; esac
    got="$(REPO_ROOT="$tmp" read_version "$kind" "$target" "$group")" || return 2
    [ "$got" = "8.8.8" ] || { fail "self-test: stamp_sites left $kind $target at '$got', expected 8.8.8"; return 1; }
  done
  derive_after="$(REPO_ROOT="$tmp" read_version cargo-package rs/crates/libs/paigasus-proto-derive/Cargo.toml)" || return 2
  [ "$derive_before" = "$derive_after" ] \
    || { fail "self-test: stamp_sites touched the publishable paigasus-proto-derive ($derive_before -> $derive_after)"; return 1; }

  # SMA-685 review F1: stamp_sites must map write_site's rc 3 (a site higher than the head) to
  # its own rc 1. It must not pass the raw code through. This is checked at the stamp_sites
  # call site, on its own pristine tree, not only at the write_site level (see F10 above).
  local tmp2 rc3=0
  tmp2="$(mktemp -d)" || { rm -rf "$tmp"; die_infra "cannot create a second scratch dir"; }
  stage_pristine_tree "$tmp2"
  _force_cargo_version "$tmp2/rs/crates/bindings/paigasus-wasm/Cargo.toml" "99.99.99" \
    || { rm -rf "$tmp2"; die_infra "cannot force the wasm binding above its head version"; }
  REPO_ROOT="$tmp2" stamp_sites >/dev/null 2>&1 || rc3=$?
  rm -rf "$tmp2"
  [ "$rc3" -eq 1 ] \
    || { fail "self-test: stamp_sites must return 1 when a site is higher than the head, got rc=$rc3"; return 1; }

  # SMA-684 §5.3: the changed-path check, on a scratch git repository with the staged tree
  # committed. A path dirty before the snapshot stays unreported. A SITES path changed after it is
  # allowed. A file in a NEW directory must be reported by its full path, which only
  # --untracked-files=all gives (the default reports `extra/`). A rename names two paths: one
  # rename moves a SITES file to a new outside path, and one moves an outside file onto a SITES
  # path. Both outside paths must be reported, and both SITES paths must not. The fixture commits,
  # so it sets the keys ci/CLAUDE.md requires (no background maintenance, no signing, no global
  # config).
  local g before viol wbody expected_viol bpos spos
  g="$(mktemp -d)" || { rm -rf "$tmp"; die_infra "cannot create a scratch dir"; }
  stage_pristine_tree "$g"
  rm -f "$g/py/uv.lock"
  printf 'outside\n' >"$g/old-b.txt"
  ( export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    git -C "$g" init -q \
      && git -C "$g" config maintenance.auto false && git -C "$g" config gc.auto 0 \
      && git -C "$g" config commit.gpgsign false && git -C "$g" config tag.gpgsign false \
      && git -C "$g" config user.name self-test && git -C "$g" config user.email self-test@invalid \
      && git -C "$g" add -A && git -C "$g" commit -q -m base ) >/dev/null 2>&1 \
    || { rm -rf "$tmp" "$g"; die_infra "cannot make the scratch git repository"; }
  printf 'x\n' >"$g/pre-existing.txt"
  before="$(REPO_ROOT="$g" dirty_paths)" \
    || { rm -rf "$tmp" "$g"; die_infra "dirty_paths failed on the scratch repository"; }
  printf 'y\n' >>"$g/pre-existing.txt"
  printf '{"version": "9.9.9"}\n' >"$g/rs/crates/bindings/paigasus-wasm/package.json"
  mkdir -p "$g/extra/new"
  printf 'z\n' >"$g/extra/new/file.txt"
  ( export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
    git -C "$g" mv rs/Cargo.lock moved-outside.txt \
      && git -C "$g" mv old-b.txt py/uv.lock ) >/dev/null 2>&1 \
    || { rm -rf "$tmp" "$g"; die_infra "cannot make the renames in the scratch git repository"; }
  viol="$(REPO_ROOT="$g" write_set_violations "$before")" \
    || { rm -rf "$tmp" "$g"; die_infra "write_set_violations failed on the scratch repository"; }
  rm -rf "$g"
  expected_viol=$'extra/new/file.txt\nmoved-outside.txt\nold-b.txt'
  [ "$viol" = "$expected_viol" ] \
    || { fail "self-test: the changed-path check reported '${viol//$'\n'/, }', expected exactly 'extra/new/file.txt, moved-outside.txt, old-b.txt'"; return 1; }
  # ...and run_write still takes the snapshot and still applies the check.
  wbody="$(_fn_body run_write)" || { fail "self-test: cannot read the body of run_write"; return 1; }
  grep -Fq 'before="$(dirty_paths)" || die_infra' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer records the dirty paths before it writes"; return 1; }
  grep -Fq 'violations="$(write_set_violations "$before")" || die_infra' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer computes the paths outside its write set"; return 1; }
  grep -Fq '|| die_infra "--write changed paths outside its write set' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer refuses a path outside its write set"; return 1; }
  # The snapshot must come BEFORE the write. A snapshot taken after stamp_sites sees the stamped
  # paths as already dirty, so it would hide every path the write changed.
  bpos="$(grep -Fn 'before="$(dirty_paths)" || die_infra' < <(printf '%s\n' "$wbody"))"
  spos="$(grep -Fn 'wrote="$(stamp_sites)"' < <(printf '%s\n' "$wbody"))"
  bpos="${bpos%%:*}"; spos="${spos%%:*}"
  { [ -n "$bpos" ] && [ -n "$spos" ] && [ "$bpos" -lt "$spos" ]; } \
    || { fail "self-test: run_write must record the dirty paths before it calls stamp_sites"; return 1; }
  # SMA-684 §5.2: run_write runs the static uv metadata check, and BEFORE uv lock. A check after
  # uv lock is too late: uv lock would already have run the build backend.
  local cpos lpos
  # `|| cpos=""`: under errexit, a grep that finds nothing would stop the script with no message.
  cpos="$(grep -Fn 'uv_static_metadata_check || uvrc=$?' < <(printf '%s\n' "$wbody"))" || cpos=""
  lpos="$(grep -Fn '( cd "$REPO_ROOT/py" && uv lock' < <(printf '%s\n' "$wbody"))" || lpos=""
  cpos="${cpos%%:*}"; lpos="${lpos%%:*}"
  { [ -n "$cpos" ] && [ -n "$lpos" ] && [ "$cpos" -lt "$lpos" ]; } \
    || { fail "self-test: run_write must run uv_static_metadata_check before uv lock"; return 1; }
  grep -Eq '^    1\) exit 1 ;;$' < <(printf '%s\n' "$wbody") \
    || { fail "self-test: run_write no longer maps the uv metadata check's rc 1 to exit 1"; return 1; }

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

# SMA-684: print the body of the shell function $1 in this file, with comment lines removed. The
# call-site pins below read a body this way. A process substitution feeds grep, because check 13
# bans a pipe into `grep -q`, and a here-string over 512 bytes deadlocks Homebrew bash 5.3.15 on a
# host whose new pipes hold 512 bytes (ci/CLAUDE.md).
_fn_body() { # $1 function name
  local all
  all="$(sed -n "/^$1() {/,/^}\$/p" "${BASH_SOURCE[0]}")" || return 2
  [ -n "$all" ] || return 2
  grep -Ev '^[[:space:]]*#' < <(printf '%s\n' "$all") || return 2
}

# SMA-684: fixture table for the napi-glue text writer (spec §5.1, §5.7), plus the call-site pins
# that keep it on the write path. The generator writes the two guard shapes that napi 3.10.4
# writes: 26 native guards and one WASI guard, as in the real index.js, so 54 version literals. A
# correct edit must equal the generator's output at the new version, byte for byte.
napi_glue_writer_self_test() {
  local tmp rc got before after entry f tgt want ino_before ino_after
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  _ngw_gen() { # $1 file  $2 version  $3 native guard count  [$4 one extra line]
    python3 - "$tmp/$1" "$2" "$3" "${4:-}" <<'PY'
import sys
p, v, n, extra = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
out = ["// prettier-ignore", "/* auto-generated by NAPI-RS */", ""]
for i in range(n):
    out += [
        "      try {",
        f"        const binding = require('@paigasus/node-bindings-t{i}')",
        f"        const bindingPackageVersion = require('@paigasus/node-bindings-t{i}/package.json').version",
        f"        if (bindingPackageVersion !== '{v}' && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {{",
        f"          throw new Error(`Native binding package version mismatch, expected {v} but got ${{bindingPackageVersion}}. You can reinstall dependencies to fix this issue.`)",
        "        }",
        "        return binding",
        "      } catch (e) {",
        "        loadErrors.push(e)",
        "      }",
    ]
out += [
    "        if (process.env.NAPI_RS_ENFORCE_VERSION_CHECK && process.env.NAPI_RS_ENFORCE_VERSION_CHECK !== '0') {",
    "          const bindingPackageVersion = require('@paigasus/node-bindings-wasm32-wasi/package.json').version",
    f"          if (bindingPackageVersion !== '{v}') {{",
    f"            throw new Error(`WASI binding package version mismatch, expected {v} but got ${{bindingPackageVersion}}. You can reinstall dependencies to fix this issue.`)",
    "          }",
    "        }",
    "module.exports.parsePrn = nativeBinding.parsePrn",
]
if extra:
    out.append(extra)
with open(p, "w", encoding="utf-8") as f:
    f.write("\n".join(out) + "\n")
PY
  }
  _ngw() { # $1 fixture  $2 version -> sets rc and got
    rc=0
    got="$(REPO_ROOT="$tmp" write_site napi-glue "$1" "$2" 2>/dev/null)" || rc=$?
  }
  _ngw_count() { # $1 file  $2 literal -> prints the number of occurrences
    python3 -c 'import sys; print(open(sys.argv[1], encoding="utf-8").read().count(sys.argv[2]))' "$tmp/$1" "$2"
  }
  _ngw_sub() { # $1 file  $2 literal  $3 replacement -> replaces the FIRST occurrence only
    python3 - "$tmp/$1" "$2" "$3" <<'PY'
import sys
p, a, b = sys.argv[1:4]
with open(p, encoding="utf-8") as f:
    s = f.read()
if a not in s:
    raise SystemExit(2)
with open(p, "w", encoding="utf-8") as f:
    f.write(s.replace(a, b, 1))
PY
  }
  _ngw_ino() { # $1 file -> prints the inode number
    python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_ino)' "$tmp/$1"
  }

  # N1: a normal bump. All 54 literals move, and the file equals the generator's 0.3.0 output.
  _ngw_gen n1.js 0.2.0 26; _ngw_gen n1.want 0.3.0 26
  _ngw n1.js 0.3.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N1 rc=$rc got='$got', expected rc 0 and 1"; return 1; }
  cmp -s "$tmp/n1.js" "$tmp/n1.want" || { fail "self-test: napi-glue N1 is not the generator's 0.3.0 output"; return 1; }
  [ "$(_ngw_count n1.js 0.3.0)" = 54 ] || { fail "self-test: napi-glue N1 does not hold 54 literals at 0.3.0"; return 1; }

  # N2: a bump that changes the length, 0.9.9 -> 0.10.0. A string compare would refuse it.
  _ngw_gen n2.js 0.9.9 26; _ngw_gen n2.want 0.10.0 26
  _ngw n2.js 0.10.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N2 rc=$rc got='$got', expected rc 0 and 1"; return 1; }
  cmp -s "$tmp/n2.js" "$tmp/n2.want" || { fail "self-test: napi-glue N2 is not the generator's 0.10.0 output"; return 1; }

  # N3: already current. Prints 0, changes no byte and keeps the mtime.
  touch -t 200001010000 "$tmp/n2.js"
  before="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/n2.js")"
  _ngw n2.js 0.10.0
  after="$(python3 -c 'import os,sys;print(os.stat(sys.argv[1]).st_mtime_ns)' "$tmp/n2.js")"
  [ "$rc" -eq 0 ] && [ "$got" = 0 ] && [ "$before" = "$after" ] \
    || { fail "self-test: napi-glue N3 rc=$rc got='$got' mtime $before -> $after"; return 1; }
  cmp -s "$tmp/n2.js" "$tmp/n2.want" || { fail "self-test: napi-glue N3 changed a byte"; return 1; }

  # N4: the NEW version string already occurs outside the two forms. It is left alone.
  _ngw_gen n4.js 0.2.0 26 "// release notes for 0.3.0"; _ngw_gen n4.want 0.3.0 26 "// release notes for 0.3.0"
  _ngw n4.js 0.3.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N4 rc=$rc got='$got'"; return 1; }
  cmp -s "$tmp/n4.js" "$tmp/n4.want" || { fail "self-test: napi-glue N4 touched the text outside the guards"; return 1; }

  # N6: the WASI guard shape alone (no env condition on the comparison line).
  _ngw_gen n6.js 0.2.0 0; _ngw_gen n6.want 0.3.0 0
  _ngw n6.js 0.3.0
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] || { fail "self-test: napi-glue N6 rc=$rc got='$got'"; return 1; }
  cmp -s "$tmp/n6.js" "$tmp/n6.want" || { fail "self-test: napi-glue N6 is not the generator's WASI output"; return 1; }

  # N9 (controller ruling P6): the write is in place. The inode stays, because a rename would break
  # the pnpm hard link (ts/CLAUDE.md).
  _ngw_gen n9.js 0.2.0 26
  ino_before="$(_ngw_ino n9.js)"
  _ngw n9.js 0.3.0
  ino_after="$(_ngw_ino n9.js)"
  [ "$rc" -eq 0 ] && [ "$got" = 1 ] && [ "$ino_before" = "$ino_after" ] \
    || { fail "self-test: napi-glue N9 rc=$rc got='$got' inode $ino_before -> $ino_after; the write must be in place"; return 1; }

  # N5 and N7: shapes the writer must refuse, each with its exit code, leaving the file unchanged.
  _ngw_gen n5.js 0.2.0 26 "// pinned at 0.2.0"         # E6: a decoy <old> outside a guard
  printf 'module.exports = {}\n' >"$tmp/e1.js"           # E1: no G1 guard at all
  _ngw_gen e2.js 0.2.0 26                                 # E2: one G2 literal lost
  _ngw_sub e2.js "expected 0.2.0 but got" "expected-0.2.0-but got" || die_infra "cannot build fixture e2"
  _ngw_gen e2b.js 0.2.0 26                                # E2: a G1 literal in a form G1 does not read
  _ngw_sub e2b.js "!== '0.2.0'" "!== '0.2.0-rc.1'" || die_infra "cannot build fixture e2b"
  _ngw_gen e3.js 0.2.0 26                                 # E3: one guard holds another version
  _ngw_sub e3.js "!== '0.2.0'" "!== '0.1.0'" || die_infra "cannot build fixture e3"
  _ngw_sub e3.js "expected 0.2.0 but got" "expected 0.1.0 but got" || die_infra "cannot build fixture e3"
  _ngw_gen l1.js 0.3.0 26                                 # L: lowering
  _ngw_gen l2.js 0.10.0 26                                # L: lowering across a length change
  _ngw_gen v1.js 0.2.0 26                                 # V: a non-plain target
  for entry in n5:0.3.0:2 e1:0.3.0:2 e2:0.3.0:2 e2b:0.3.0:2 e3:0.3.0:3 l1:0.2.0:3 l2:0.9.9:3 v1:0.3.0-rc.1:2; do
    IFS=':' read -r f tgt want <<<"$entry"
    cp "$tmp/$f.js" "$tmp/$f.before"
    _ngw "$f.js" "$tgt"
    [ "$rc" -eq "$want" ] || { fail "self-test: napi-glue $f must exit $want, got rc=$rc"; return 1; }
    cmp -s "$tmp/$f.js" "$tmp/$f.before" || { fail "self-test: napi-glue $f changed the file although refused"; return 1; }
  done

  # N8: E4 and E5 cannot fire on a correct writer, so the verify mode gets an edit that no correct
  # writer makes. The positive control first: a correct edit verifies.
  _ngw_gen x_old.js 0.2.0 26; _ngw_gen x_ok.js 0.3.0 26
  napi_glue_py verify "$tmp/x_old.js" "$tmp/x_ok.js" 0.3.0 2>/dev/null \
    || { fail "self-test: napi-glue verify refused a correct edit"; return 1; }
  cp "$tmp/x_ok.js" "$tmp/x_e4.js"
  _ngw_sub x_e4.js "!== '0.3.0'" "!== '0.2.0'" || die_infra "cannot build fixture x_e4"
  rc=0; napi_glue_py verify "$tmp/x_old.js" "$tmp/x_e4.js" 0.3.0 2>/dev/null || rc=$?
  [ "$rc" -eq 2 ] || { fail "self-test: napi-glue E4 (one G1 literal left behind) rc=$rc, expected 2"; return 1; }
  # Controller ruling P5: the G2 half of E4. Only a G2 literal stays at the old version.
  cp "$tmp/x_ok.js" "$tmp/x_e4g2.js"
  _ngw_sub x_e4g2.js "expected 0.3.0 but got" "expected 0.2.0 but got" || die_infra "cannot build fixture x_e4g2"
  rc=0; napi_glue_py verify "$tmp/x_old.js" "$tmp/x_e4g2.js" 0.3.0 2>/dev/null || rc=$?
  [ "$rc" -eq 2 ] || { fail "self-test: napi-glue E4 (one G2 literal left behind) rc=$rc, expected 2"; return 1; }
  cp "$tmp/x_ok.js" "$tmp/x_e5.js"; printf 'x' >>"$tmp/x_e5.js"
  rc=0; napi_glue_py verify "$tmp/x_old.js" "$tmp/x_e5.js" 0.3.0 2>/dev/null || rc=$?
  [ "$rc" -eq 2 ] || { fail "self-test: napi-glue E5 (a byte outside the literals) rc=$rc, expected 2"; return 1; }

  # Call-site pins (spec §5.7). write_site keeps its arm, and stamp_sites passes the kind to it.
  grep -Eq '^    napi-glue\)$' < <(_fn_body write_site) \
    || { fail "self-test: write_site has no napi-glue) arm"; return 1; }
  grep -Eq '^      ([a-z-]+[|])*napi-glue([|][a-z-]+)*\) ;;$' < <(_fn_body stamp_sites) \
    || { fail "self-test: the stamp_sites filter does not name napi-glue"; return 1; }
  # Controller ruling P5: the write mode of napi_glue_py calls verify() before it writes.
  grep -Eq '^    verify\(old, new, target\)$' < <(_fn_body napi_glue_py) \
    || { fail "self-test: napi_glue_py write mode does not call verify()"; return 1; }

  # run_write compiles nothing (spec A1, A3). Comment lines are removed first. A bare \bnode\b
  # would match paigasus-node-bindings, so a command word is matched by its neighbours instead.
  local body
  body="$(_fn_body run_write)" || { fail "self-test: cannot read the body of run_write"; return 1; }
  if grep -Eq '(^|[[:space:];&|(])(pnpm|npx|napi|node)([[:space:];&|)]|$)' < <(printf '%s\n' "$body"); then
    fail "self-test: run_write names pnpm, npx, napi or node as a command word; --write must compile nothing (SMA-684)"
    return 1
  fi

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

# SMA-684 §5.2: fixture table for the static uv metadata check. Each case is a small uv workspace
# under its own scratch REPO_ROOT: one member with a path source (the maturin crate's shape), and
# one member that SITES does not name (the dormant shape of paigasus-ml and paigasus-workflows).
uv_static_metadata_self_test() {
  local tmp rc listed out
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  _usm_tree() { # $1 case dir -> a clean workspace
    local r="$tmp/$1"
    mkdir -p "$r/py/packages/a" "$r/py/packages/dormant" "$r/rs/b"
    printf '[tool.uv.workspace]\nmembers = ["packages/*"]\n' >"$r/py/pyproject.toml"
    printf '[project]\nname = "a"\nversion = "0.1.0"\ndependencies = ["b==0.1.0"]\n\n[tool.uv.sources]\nb = { path = "../../../rs/b" }\n' >"$r/py/packages/a/pyproject.toml"
    printf '[project]\nname = "dormant"\nversion = "0.0.0"\ndependencies = []\n' >"$r/py/packages/dormant/pyproject.toml"
    printf '[project]\nname = "b"\nversion = "0.1.0"\n\n[build-system]\nrequires = ["maturin>=1.9.6,<2"]\nbuild-backend = "maturin"\n' >"$r/rs/b/pyproject.toml"
  }
  _usm() { # $1 case dir -> sets rc
    rc=0
    REPO_ROOT="$tmp/$1" uv_static_metadata_check >/dev/null 2>&1 || rc=$?
  }

  # U0: the clean tree passes.
  _usm_tree u0; _usm u0
  [ "$rc" -eq 0 ] || { fail "self-test: uv-static U0 (clean) rc=$rc, expected 0"; return 1; }
  # U0b: --list names every file the check reads, the path source included. stage_pristine_tree
  # stages exactly this list, so a file missing here is a file the staged trees lack.
  listed="$(REPO_ROOT="$tmp/u0" uv_static_metadata_check --list)" \
    || { fail "self-test: uv-static --list failed on the clean tree"; return 1; }
  [ "$listed" = "$(printf 'py/packages/a/pyproject.toml\npy/packages/dormant/pyproject.toml\npy/pyproject.toml\nrs/b/pyproject.toml')" ] \
    || { fail "self-test: uv-static --list printed '$listed'"; return 1; }
  # U1: dynamic version in a member that SITES does not name.
  _usm_tree u1
  printf '[project]\nname = "dormant"\ndynamic = ["version"]\ndependencies = []\n' >"$tmp/u1/py/packages/dormant/pyproject.toml"
  _usm u1; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U1 (dynamic version) rc=$rc, expected 1"; return 1; }
  # U2: dynamic dependencies in a member.
  _usm_tree u2
  printf '[project]\nname = "a"\nversion = "0.1.0"\ndynamic = ["dependencies"]\n\n[tool.uv.sources]\nb = { path = "../../../rs/b" }\n' >"$tmp/u2/py/packages/a/pyproject.toml"
  _usm u2; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U2 (dynamic dependencies) rc=$rc, expected 1"; return 1; }
  # U3: dynamic optional-dependencies in the path source.
  _usm_tree u3
  printf '[project]\nname = "b"\nversion = "0.1.0"\ndynamic = ["optional-dependencies"]\n\n[build-system]\nrequires = ["maturin>=1.9.6,<2"]\nbuild-backend = "maturin"\n' >"$tmp/u3/rs/b/pyproject.toml"
  _usm u3; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U3 (dynamic optional-dependencies) rc=$rc, expected 1"; return 1; }
  # U4: a member with no [project] table: uv would run its build backend to read its metadata.
  _usm_tree u4
  printf '[build-system]\nrequires = ["setuptools"]\n' >"$tmp/u4/py/packages/dormant/pyproject.toml"
  _usm u4; [ "$rc" -eq 1 ] || { fail "self-test: uv-static U4 (no [project]) rc=$rc, expected 1"; return 1; }
  # U5: a dynamic field outside the three named ones is allowed (the check is not over-broad).
  _usm_tree u5
  printf '[project]\nname = "dormant"\nversion = "0.0.0"\ndependencies = []\ndynamic = ["classifiers"]\n' >"$tmp/u5/py/packages/dormant/pyproject.toml"
  _usm u5; [ "$rc" -eq 0 ] || { fail "self-test: uv-static U5 (dynamic classifiers) rc=$rc, expected 0"; return 1; }
  # U6: the message names the file and the field.
  out="$(REPO_ROOT="$tmp/u1" uv_static_metadata_check 2>&1 >/dev/null)" || true
  grep -Fq "py/packages/dormant/pyproject.toml: [project].dynamic lists 'version'" < <(printf '%s\n' "$out") \
    || { fail "self-test: uv-static U6 message does not name the file and the field: '$out'"; return 1; }
  # U7: a malformed member is an infrastructure failure, not a verdict.
  _usm_tree u7
  printf '[project\n' >"$tmp/u7/py/packages/dormant/pyproject.toml"
  _usm u7; [ "$rc" -eq 2 ] || { fail "self-test: uv-static U7 (malformed) rc=$rc, expected 2"; return 1; }
  # U8: the production path. The real run_check, on a staged copy of the real tree, with a dynamic
  # version in the staged paigasus-ml (a uv member that SITES does not name). rc 2 here means the
  # staging lacks a uv file; rc 0 means run_check no longer calls the check.
  mkdir "$tmp/st"
  stage_pristine_tree "$tmp/st"
  mkdir -p "$tmp/st/py/packages/paigasus-ml"
  printf '[project]\nname = "paigasus-ml"\ndynamic = ["version"]\ndependencies = []\n' >"$tmp/st/py/packages/paigasus-ml/pyproject.toml"
  rc=0; ( REPO_ROOT="$tmp/st" run_check ) >/dev/null 2>&1 || rc=$?
  [ "$rc" -eq 1 ] || { fail "self-test: uv-static U8 run_check on a staged tree with a dynamic member rc=$rc, expected 1"; return 1; }

  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

run_self_tests() {
  SELF_TESTS_RAN=0
  local defs
  defs="$(grep -cE '^[a-z_]+_self_test\(\) \{$' "${BASH_SOURCE[0]}")" || die_infra "cannot count self-test definitions"
  [ "$defs" -eq "$SELF_TEST_COUNT" ] \
    || die_infra "found $defs *_self_test definitions, expected $SELF_TEST_COUNT"
  site_verdict_self_test
  lock_reader_self_test
  cargo_package_writer_self_test
  stamp_sites_self_test
  napi_glue_writer_self_test
  uv_static_metadata_self_test
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
    actual="$(read_version "$kind" "$target" "$group")" || return 2
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
  # SMA-684 §5.2: uv lock in --write must never need a build backend. Explicit status routing,
  # not errexit, for the same reason as read_version above.
  local uvrc=0
  uv_static_metadata_check || uvrc=$?
  case "$uvrc" in
    0) printf 'uv workspace: every local package declares static metadata\n' ;;
    1) rc=1 ;;
    *) return 2 ;;
  esac
  if [ "$rc" -eq 0 ]; then
    printf '== all %d version-lockstep sites agree ==\n' "$checked"
  fi
  return "$rc"
}

# Stage exactly the files the SITES table names, plus rs/Cargo.toml (which the cargo-wsdep
# kind reads by name rather than by path), into a PRISTINE copy at $1. Deriving the list from
# SITES rather than from a hand-written glob keeps the control honest when a site is added: a
# new site is staged automatically, so the control cannot quietly stop covering it.
#
# A shared function called once per drift — rather than one scratch dir reused across both
# drifts — because reuse left the FIRST drift still present in the tree while the SECOND
# run_check ran: run_check's loop fails on any mismatched site and keeps checking the rest, so
# the second run_check's rc=1 was guaranteed by the still-present first drift regardless of
# whether the lock-row drift landed at all (SMA-577 review, Critical). Each drift now gets its
# own pristine tree, so a later drift added to this control automatically gets isolation
# instead of silently inheriting the same bug.
#
# SMA-685 review (round 2): on a staging failure this removes its OWN $dest before it dies.
# `die_infra` exits the whole process. An `exit` does not run a caller's RETURN trap, so a
# caller-owned cleanup would not fire. This function owns $dest, so it cleans $dest itself.
stage_pristine_tree() { # $1 destination dir
  local dest="$1" entry kind target uvfiles
  # SMA-684: run_check also reads the uv workspace pyproject files (uv_static_metadata_check), so
  # the staged tree carries the same list that check reads, derived from the check itself.
  uvfiles="$(uv_static_metadata_check --list)" \
    || { rm -rf "$dest"; die_infra "cannot list the uv workspace pyproject files to stage"; }
  {
    printf 'rs/Cargo.toml\n'
    printf '%s\n' "$uvfiles"
    for entry in "${SITES[@]}"; do
      IFS='|' read -r _ kind target <<<"$entry"
      [ "$kind" = cargo-wsdep ] || printf '%s\n' "$target"
    done
  } | sort -u | ( cd "$REPO_ROOT" && tar -cf - -T - ) | ( cd "$dest" && tar -xf - ) \
    || { rm -rf "$dest"; die_infra "cannot stage a scratch copy of the version-carrying files"; }
}

# Drift ONE site in its OWN pristine scratch tree, and assert the checker reports red. Driving
# the real run_check (not a reimplementation) is what makes this a control rather than a
# second, differently-wrong checker.
negative_control() {
  local tmp1 tmp2 tmp3
  tmp1="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  tmp2="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  tmp3="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp1' '$tmp2' '$tmp3'" RETURN

  stage_pristine_tree "$tmp1"

  # Drift site 13 (@paigasus/node-bindings) to a version no group member carries.
  python3 - "$tmp1/rs/crates/bindings/paigasus-node-bindings/package.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["version"] = "99.99.99"
json.dump(d, open(p, "w"), indent=2)
PY

  local ec=0
  REPO_ROOT="$tmp1" run_check >/dev/null 2>&1 || ec=$?
  if [ "$ec" -eq 2 ]; then
    fail "negative control: run_check hit an infrastructure failure (exit 2) instead of
      reporting the drift. The scratch staging is incomplete or a site went unreadable —
      that is a broken control, not proof the gate can report red."
    return 1
  fi
  if [ "$ec" -ne 1 ]; then
    fail "negative control: a drifted site was ACCEPTED (run_check exited $ec, expected 1).
      The gate can no longer report red and is green exactly when it matters."
    return 1
  fi
  # First drift correctly reported red — fall through (rather than returning) so the second,
  # lock-row drift below is exercised too, instead of returning early on the first success.
  printf '== negative control: version-lockstep reported red as expected ==\n'

  stage_pristine_tree "$tmp2"

  # Second drift: a LOCK row, run against its OWN pristine tree (SMA-577 review, Critical) so
  # this run_check's red proves the lock drift specifically — not a leftover from the first
  # drift above, which no longer exists in $tmp2. The packagejson drift above exercises no
  # lock handler, so without this the new LOCK_MEMBERS table has no end-to-end control
  # coverage.
  python3 - "$tmp2/rs/Cargo.lock" <<'PY'
import re, sys
p = sys.argv[1]
text = open(p, encoding="utf-8").read()
# Drift ONE proto member so the set is non-uniform -> the reader must print "".
text = re.sub(
    r'(\[\[package\]\]\nname = "paigasus-proto-derive"\nversion = ")[^"]+(")',
    r"\g<1>99.99.99\g<2>", text, count=1)
open(p, "w", encoding="utf-8").write(text)
PY

  local ec2=0
  REPO_ROOT="$tmp2" run_check >/dev/null 2>&1 || ec2=$?
  if [ "$ec2" -eq 2 ]; then
    fail "negative control: run_check hit an infrastructure failure (exit 2) instead of
      reporting the lock-row drift. The scratch staging is incomplete or a site went
      unreadable — that is a broken control, not proof the gate can report red."
    return 1
  fi
  if [ "$ec2" -ne 1 ]; then
    fail "negative control: a drifted cargo-lock member was ACCEPTED (run_check exited $ec2, expected 1).
      The LOCK_MEMBERS table or the cargo-lock reader can no longer report red."
    return 1
  fi
  printf '== negative control: version-lockstep reported red on both a packagejson and a lock drift ==\n'

  stage_pristine_tree "$tmp3"

  # Third drift (SMA-685): a cargo-package binding manifest. README L1 recorded that this kind had
  # no end-to-end drift, and a release-plz bump of the kernel alone produces exactly this shape.
  _force_cargo_version "$tmp3/rs/crates/bindings/paigasus-wasm/Cargo.toml" "99.99.99" \
    || { rm -rf "$tmp1" "$tmp2" "$tmp3"; die_infra "cannot drift the cargo-package binding manifest"; }

  local ec3=0
  REPO_ROOT="$tmp3" run_check >/dev/null 2>&1 || ec3=$?
  if [ "$ec3" -eq 2 ]; then
    fail "negative control: run_check hit an infrastructure failure (exit 2) instead of
      reporting the cargo-package drift."
    return 1
  fi
  if [ "$ec3" -ne 1 ]; then
    fail "negative control: a drifted binding manifest was ACCEPTED (run_check exited $ec3, expected 1)."
    return 1
  fi
  printf '== negative control: version-lockstep reported red on a packagejson, a lock and a cargo-package drift ==\n'
  return 0
}

cargo_publish_false() { # $1 target -> prints 1 if Cargo `publish` is false or [], else 0
  # SMA-685 review F7: `publish.workspace = true` inherits its real value from the workspace
  # root, which this function does not read. Treating it as "publishable" would be a guess,
  # not a fact. This arm fails closed (rc 2) on that shape, and on any other type it does not
  # recognize, instead of silently guessing 0.
  local abs="$REPO_ROOT/$1"
  [ -r "$abs" ] || die_infra "cannot read $1"
  python3 - "$abs" <<'PY'
import sys, tomllib
p = sys.argv[1]
try:
    pub = tomllib.load(open(p, "rb"))["package"].get("publish", True)
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
if pub is False or pub == []:
    print(1)
elif pub is True or (isinstance(pub, list) and len(pub) > 0):
    print(0)
else:
    print(f"malformed {p}: cannot read publish = {pub!r} (workspace inheritance not resolved)", file=sys.stderr)
    sys.exit(2)
PY
}

# SMA-684: the version literals of the committed napi glue, written as TEXT (spec §5.1). `napi
# build` wrote them before, and it compiled the binding crate, the kernel and every build script
# and proc macro in their graph, in the release-PR job, where every step can read the App private
# key. This writer compiles nothing.
#
# Two literal forms carry the version, and the version is the only capture group:
#   G1  bindingPackageVersion !== '<X.Y.Z>'
#   G2  expected <X.Y.Z> but got
# Modes:
#   write <file> <version>         every check in memory, then an in-place write (open for
#                                  writing, never a rename: a rename breaks the pnpm hard link,
#                                  ts/CLAUDE.md). Prints 1 if it wrote, 0 if already current.
#   verify <old> <new> <version>   E4 and E5 only. The self-test drives this mode with an edit
#                                  that no correct writer makes.
# Exit codes follow write_site's contract (see stamp_sites below): 0 wrote or already current;
# 3 the repo is wrong (E3: the guards disagree; L: the file is higher than the head), which
# stamp_sites maps to rc 1; 2 for every other failure (V, E1, E2, E4, E5, E6, an unreadable
# file). E3 exits 3 and not 1 here: a python traceback also exits 1, and stamp_sites must read
# that as an infrastructure failure.
napi_glue_py() { # write <file> <version> | verify <old-file> <new-file> <version>
  python3 - "$@" <<'PY'
import re, sys

VER = r"[0-9]+\.[0-9]+\.[0-9]+"
G1 = re.compile(r"bindingPackageVersion !== '(" + VER + r")'")
G2 = re.compile(r"expected (" + VER + r") but got")
G1_ANY = "bindingPackageVersion !== '"
G2_ANY = re.compile(r"expected \S+ but got")
MASK = "\x00V\x00"


def fatal(msg):
    print(f"FATAL: {msg}", file=sys.stderr)
    raise SystemExit(2)


def repo_wrong(msg):
    print(f"FAIL: {msg}", file=sys.stderr)
    raise SystemExit(3)


def plain(v, what):
    m = re.fullmatch(r"([0-9]+)\.([0-9]+)\.([0-9]+)", v)
    if m is None:
        fatal(f"{what} version '{v}' is not plain X.Y.Z")
    return tuple(int(x) for x in m.groups())


def read_text(p):
    try:
        with open(p, "rb") as f:
            return f.read().decode("utf-8")
    except (OSError, UnicodeDecodeError) as e:
        fatal(f"cannot read {p} as UTF-8: {e}")


def masked(s):
    s = G1.sub(lambda m: "bindingPackageVersion !== '" + MASK + "'", s)
    return G2.sub(lambda m: "expected " + MASK + " but got", s)


def verify(old, new, target):
    # E4: every literal in the new text holds the target, and no literal appeared or vanished.
    old1, old2 = G1.findall(old), G2.findall(old)
    new1, new2 = G1.findall(new), G2.findall(new)
    if len(new1) != len(old1) or len(new2) != len(old2) or any(v != target for v in new1 + new2):
        fatal(f"E4: after the edit, not every guard literal holds {target}; writer defect")
    # E5: masks, not a reverse edit on positions. A reverse edit shares code with the forward
    # edit, and its positions move when the version length changes (0.9.9 -> 0.10.0).
    if masked(old) != masked(new):
        fatal("E5: the edit changed a byte outside the guard literals; writer defect")


def write(path, target):
    # V: the target is plain X.Y.Z, the same rule as the cargo-package arm's plain().
    head = plain(target, "head")
    old = read_text(path)
    g1, g2 = G1.findall(old), G2.findall(old)
    # E1: at least one G1 literal.
    if not g1:
        fatal(f"E1: {path} has no `bindingPackageVersion !== '<X.Y.Z>'` guard; the napi format changed")
    # E2: G1 and G2 agree in count, and no literal of either form escapes the strict pattern.
    n1_any, n2_any = old.count(G1_ANY), len(G2_ANY.findall(old))
    if len(g1) != len(g2) or n1_any != len(g1) or n2_any != len(g2):
        fatal(f"E2: {path} has {len(g1)} G1 and {len(g2)} G2 literals, {n1_any} and {n2_any} in any form; the napi format changed")
    # E6: each version string the guards hold occurs exactly twice per guard in the whole file.
    values = sorted(set(g1 + g2))
    found = sum(old.count(v) for v in values)
    if found != 2 * len(g1):
        fatal(f"E6: {path} holds {values} {found} times, expected {2 * len(g1)} (two per guard); napi added or removed a version-bearing literal")
    # E3: one value across every literal.
    if len(values) != 1:
        repo_wrong(f"{path}: the guards hold more than one version: {values}")
    cur = values[0]
    # L: never lower. Integer tuples, not strings: "0.10.0" < "0.9.9" as strings.
    if plain(cur, "site") > head:
        repo_wrong(f"{path} is at {cur}, higher than the head {target}; not lowered")
    if cur == target:
        print(0)
        raise SystemExit(0)
    new = G1.sub(lambda m: "bindingPackageVersion !== '" + target + "'", old)
    new = G2.sub(lambda m: "expected " + target + " but got", new)
    verify(old, new, target)
    with open(path, "wb") as f:
        f.write(new.encode("utf-8"))
    print(1)


args = sys.argv[1:]
if len(args) == 3 and args[0] == "write":
    write(args[1], args[2])
elif len(args) == 4 and args[0] == "verify":
    verify(read_text(args[1]), read_text(args[2]), args[3])
else:
    fatal(f"usage: napi_glue_py write <file> <version> | verify <old> <new> <version>, got {args}")
PY
}

# SMA-684 §5.2: `uv lock` runs no build backend only while uv can read every local package's
# metadata without a build (spec F7). A `dynamic` version, dependencies or optional-dependencies
# field makes `uv lock` run the build backend, and for paigasus-py-bindings that backend is
# maturin, which compiles Rust in the release-PR job. The pyproject reader above covers only the
# three SITES pyproject files; this check reads every uv workspace member (members glob of
# py/pyproject.toml, minus `exclude`) and every [tool.uv.sources] path source, transitively. A
# member with no [project] table and a path source that is not a directory are violations too:
# uv would build either to read its metadata. With --list it prints the files it reads instead,
# so stage_pristine_tree can stage them.
uv_static_metadata_check() { # [--list] -> rc 0 clean | 1 a violation | 2 cannot read
  python3 - "$REPO_ROOT" "$@" <<'PY'
import glob, os, sys, tomllib
from fnmatch import fnmatch

root, args = sys.argv[1], sys.argv[2:]
listing = args == ["--list"]
if args and not listing:
    print(f"INFRA: unknown argument(s) {args}", file=sys.stderr)
    raise SystemExit(2)
py = os.path.join(root, "py")
ws_file = os.path.normpath(os.path.join(py, "pyproject.toml"))


def rel(p):
    return os.path.relpath(p, root)


def load(p):
    try:
        with open(p, "rb") as f:
            return tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        print(f"INFRA: cannot read {rel(p)}: {e}", file=sys.stderr)
        raise SystemExit(2) from None


ws = load(ws_file)
uvws = ws.get("tool", {}).get("uv", {}).get("workspace")
if not isinstance(uvws, dict) or not uvws.get("members"):
    print("INFRA: py/pyproject.toml declares no [tool.uv.workspace] members", file=sys.stderr)
    raise SystemExit(2)
queue = [ws_file]
for pat in uvws["members"]:
    dirs = [d for d in sorted(glob.glob(os.path.join(py, pat))) if os.path.isdir(d)]
    if not dirs:
        print(f"INFRA: the uv workspace member glob {pat!r} matches no directory", file=sys.stderr)
        raise SystemExit(2)
    for d in dirs:
        if not any(fnmatch(os.path.relpath(d, py), e) for e in uvws.get("exclude", [])):
            queue.append(os.path.normpath(os.path.join(d, "pyproject.toml")))

seen, violations = set(), []
while queue:
    p = queue.pop(0)
    if p in seen:
        continue
    seen.add(p)
    doc = load(p)
    proj = doc.get("project")
    if p != ws_file and not isinstance(proj, dict):
        violations.append(f"{rel(p)}: no [project] table, so uv would run the build backend to read its metadata")
    if isinstance(proj, dict):
        dynamic = proj.get("dynamic", [])
        for field in ("version", "dependencies", "optional-dependencies"):
            if field in dynamic:
                violations.append(f"{rel(p)}: [project].dynamic lists '{field}', so uv lock would run the build backend")
    sources = doc.get("tool", {}).get("uv", {}).get("sources", {})
    for name, src in sources.items():
        for entry in src if isinstance(src, list) else [src]:
            if isinstance(entry, dict) and "path" in entry:
                target = os.path.normpath(os.path.join(os.path.dirname(p), entry["path"]))
                if os.path.isdir(target):
                    queue.append(os.path.join(target, "pyproject.toml"))
                else:
                    violations.append(f"{rel(p)}: [tool.uv.sources] {name} path {entry['path']!r} is not a directory, so uv would build it")

if listing:
    for p in sorted(seen):
        print(rel(p))
    raise SystemExit(0)
for v in violations:
    print(f"FAIL: {v}", file=sys.stderr)
raise SystemExit(1 if violations else 0)
PY
}

# SMA-685: the per-site loop of --write, split out of run_write so the self-test runs the SAME
# loop on a staged tree. It writes the pyproject, pyproject-dep, packagejson and napi-glue sites
# (napi-glue since SMA-684), and the
# cargo-package sites that are NOT the group head and whose Cargo manifest says
# `publish = false`. release-plz 0.3.158 never writes those (READ, updater.rs:283-302). A
# publishable non-head (paigasus-proto-derive) is left to release-plz, so --check still sees a
# version_group fault there. Prints the count of changed sites.
#
# Writer contract (SMA-685 review F1): write_site's own exit codes are 0 (wrote, or already
# correct), 3 (refuses to lower an existing higher version), or 2 (any other failure). Passing
# a raw write_site status straight through would break this script's 0/1/2 header contract —
# a python traceback (rc 1) or a missing python3 (rc 127) would then read as "assertion
# failed" instead of "infrastructure failed". stamp_sites maps write_site's rc 3 to its own
# rc 1 (the repo is wrong: a site is higher than the head). It maps every other non-zero
# write_site status to rc 2 (infrastructure failed).
stamp_sites() {
  local wrote=0 group kind target head expected changed pf rc
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    head="${SOURCE_OF_TRUTH[$group]}"
    case "$kind" in
      pyproject|pyproject-dep|packagejson|napi-glue) ;;
      cargo-package)
        [ "$target" != "$head" ] || continue
        pf="$(cargo_publish_false "$target")" || return 2
        [ "$pf" = 1 ] || continue
        ;;
      *) continue ;;
    esac
    # Explicit status handling rather than errexit: stamp_sites may be called on the left of
    # `||`, which suspends errexit (same discipline as run_check, SMA-576).
    expected="$(read_version cargo-package "$head")" || return 2
    rc=0
    changed="$(write_site "$kind" "$target" "$expected")" || rc=$?
    if [ "$rc" -eq 3 ]; then
      return 1
    elif [ "$rc" -ne 0 ]; then
      return 2
    fi
    wrote=$((wrote + changed))
  done
  printf '%d\n' "$wrote"
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
    cargo-package)
      # SMA-685: the [package] version of a Cargo manifest, edited in place (no TOML
      # round-trip, so no unrelated churn). release-plz 0.3.158 never writes the version of a
      # crate whose Cargo manifest says `publish = false`, version_group or not (READ,
      # updater.rs:283-302), so --write stamps those. stamp_sites decides WHICH sites; this arm
      # only edits one file. It fails closed (rc 2) on any shape it does not understand.
      # It refuses to lower a version with rc 3 (SMA-685 review F1). This is write_site's own
      # exit code, not the script's 0/1/2 header contract. A raw python error would otherwise
      # exit rc 1, and a missing python3 would exit rc 127. Both must read as an infrastructure
      # failure, not as "the repo is wrong". stamp_sites maps write_site's rc 3 to its own
      # rc 1. It maps any other non-zero write_site status to rc 2.
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 - "$abs" "$version" <<'PY'
import re, sys, tomllib

def fatal(msg):
    print(f"FATAL: {msg}", file=sys.stderr); raise SystemExit(2)

def plain(v, what):
    m = re.fullmatch(r"([0-9]+)\.([0-9]+)\.([0-9]+)", v)
    if m is None:
        fatal(f"{what} version '{v}' is not plain X.Y.Z")
    return tuple(int(x) for x in m.groups())

p, v = sys.argv[1], sys.argv[2]
head = plain(v, "head")
raw = open(p, "rb").read()
try:
    s = raw.decode("utf-8")
    old_doc = tomllib.loads(s)
except Exception as e:
    fatal(f"malformed {p}: {e}")
hdrs = list(re.finditer(r"(?m)^[ \t]*\[package\][ \t]*(?:#[^\r\n]*)?\r?$", s))
if len(hdrs) != 1:
    fatal(f"{p}: expected one [package] table header, found {len(hdrs)}")
start = hdrs[0].end()
nxt = re.search(r"(?m)^[ \t]*\[", s[start:])
end = start + nxt.start() if nxt else len(s)
body = s[start:end]
keys = list(re.finditer(r"(?m)^[ \t]*version[ \t]*[=.]", body))
if len(keys) != 1:
    fatal(f"{p}: expected one version key in [package], found {len(keys)}")
vm = re.search(r'(?m)^([ \t]*version[ \t]*=[ \t]*")([^"\r\n]*)(")', body)
if vm is None:
    fatal(f"{p}: the [package] version is not a literal string")
cur = plain(vm.group(2), "site")
if cur > head:
    # rc 3, not rc 1 (SMA-685 review F1). stamp_sites maps this specific refusal to its
    # own rc 1. A different write_site failure, such as a python error or a missing
    # python3, must not also read as rc 1. Otherwise the caller cannot tell a repo
    # fault from a broken check.
    print(f"FAIL: {p} is at {vm.group(2)}, higher than the head {v}; not lowered", file=sys.stderr)
    raise SystemExit(3)
if cur == head:
    print(0); raise SystemExit(0)
a, b = start + vm.start(2), start + vm.end(2)
new = s[:a] + v + s[b:]
try:
    new_doc = tomllib.loads(new)
except Exception as e:
    fatal(f"{p}: the edit made invalid TOML: {e}")
old_doc.setdefault("package", {})["version"] = v
if new_doc != old_doc:
    fatal(f"{p}: the edit changed more than package.version")
open(p, "wb").write(new.encode("utf-8"))
print(1)
PY
      ;;
    napi-glue)
      # SMA-684: the version literals of the committed napi glue, edited as text. napi_glue_py
      # holds the checks and the exit codes; this arm only names the file.
      [ -r "$abs" ] || die_infra "cannot read $target"
      napi_glue_py write "$abs" "$version"
      ;;
    *) printf '0' ;;   # regeneration-owned kinds (cargo-wsdep, the two locks) are not written here
  esac
}

# SMA-684 §5.3: the dirty and untracked paths of the work tree at $REPO_ROOT, one per line,
# sorted. `-z` keeps a path with a space or a quote intact. `--untracked-files=all` names every
# untracked FILE: the default collapses a new directory to `dir/`, which would hide a file inside
# a directory that was already untracked. A rename or copy names both of its paths.
dirty_paths() {
  python3 - "$REPO_ROOT" <<'PY'
import subprocess, sys
try:
    out = subprocess.run(
        ["git", "-C", sys.argv[1], "status", "--porcelain=v1", "-z", "--untracked-files=all"],
        check=True, capture_output=True).stdout.decode("utf-8", "surrogateescape")
except (OSError, subprocess.CalledProcessError) as e:
    print(f"INFRA: git status failed in {sys.argv[1]}: {e}", file=sys.stderr)
    raise SystemExit(2) from None
parts, paths, i = out.split("\0"), set(), 0
while i < len(parts):
    entry = parts[i]
    i += 1
    if not entry:
        continue
    xy, path = entry[:2], entry[3:]
    paths.add(path)
    if "R" in xy or "C" in xy:
        paths.add(parts[i])
        i += 1
for p in sorted(paths):
    print(p)
PY
}

# SMA-684 §5.3: print every path that is dirty now, was not dirty in $1 (dirty_paths' output from
# before the write), and is not in the write set. The write set is the SITES paths, rs/Cargo.lock
# and py/uv.lock among them; a cargo-wsdep row names a crate, not a file, so it adds no path. The
# stamp step runs `git add -A`, so this set is the boundary of what the release-PR job commits.
write_set_violations() { # $1 the dirty_paths output from before the write
  local after entry kind target allowed=""
  after="$(dirty_paths)" || return 2
  for entry in "${SITES[@]}"; do
    IFS='|' read -r _ kind target <<<"$entry"
    [ "$kind" = cargo-wsdep ] || allowed="$allowed$target"$'\n'
  done
  python3 - "$1" "$after" "$allowed" <<'PY'
import sys
before, after, allowed = (set(filter(None, a.split("\n"))) for a in sys.argv[1:4])
for p in sorted(after - before - allowed):
    print(p)
PY
}

run_write() {
  local wrote rc=0 before violations uvrc
  # SMA-684 §5.3: the paths that were dirty before this run. Every path that is new after it must
  # be in the write set; write_set_violations below holds that.
  before="$(dirty_paths)" || die_infra "cannot list the dirty paths before --write"
  wrote="$(stamp_sites)" || rc=$?
  [ "$rc" -eq 0 ] || return "$rc"

  # Regenerate the two derived lock files (SITES rows 16, 17, 19 and 20: kernel's and proto's
  # cargo-lock and uv-lock rows each point at the same file, so four rows resolve to two files).
  # Each lock file is owned by its tool. Neither command runs a build script or a build backend
  # (spec F6, F7; --check asserts the static uv metadata that F7 needs). Row 18, the napi glue,
  # is written by stamp_sites above through write_site's napi-glue arm (SMA-684). This function
  # compiles nothing: the release-PR job runs it, and every step of that job can read the App
  # private key. napi_glue_writer_self_test pins that no build command comes back here.
  ( cd "$REPO_ROOT/rs" && cargo update -w --offline >/dev/null 2>&1 ) \
    || ( cd "$REPO_ROOT/rs" && cargo update -w >/dev/null ) \
    || die_infra "cargo update -w failed (site 16)"
  # SMA-684 §5.2: uv lock runs no build backend only while every local package declares static
  # metadata. run_check asserts that too, but a path source outside py/packages/* is not an input
  # of repo:version-lockstep, so a later PR could add `dynamic` there unseen. Check it here again,
  # right before uv lock, with the same status routing as run_check. Print nothing on success: the
  # stamp step's output stays the same.
  uvrc=0
  uv_static_metadata_check || uvrc=$?
  case "$uvrc" in
    0) ;;
    1) exit 1 ;;
    *) exit 2 ;;
  esac
  ( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"

  violations="$(write_set_violations "$before")" || die_infra "cannot list the changed paths after --write"
  [ -z "$violations" ] \
    || die_infra "--write changed paths outside its write set (the SITES paths): ${violations//$'\n'/, }"

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
