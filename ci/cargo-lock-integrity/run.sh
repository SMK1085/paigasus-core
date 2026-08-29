#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-601 — assert that rs/Cargo.lock satisfies every workspace manifest.
#
# WHY THIS IS A ci.yml STEP AND NOT A repo:* MOON TASK. Dependabot cargo PRs repeatedly ship a
# truncated lock (PRs 83, 96, 140, 149, 181). `moon ci` was green on all of them because an
# UNLOCKED cargo invocation re-resolves and rewrites an inconsistent lock in place, mid-run,
# before any --locked task reads it: measured on PR 181's 72c0ddb52, `cargo tree` and `cargo deny`
# each rewrote the lock from 176 packages to 548 and exited 0, both starting at 06:37:55, twelve
# seconds before the first --locked task. A Moon task would race those repairers. This step runs
# BEFORE the `moon ci` step, when nothing has run yet, so the working tree still holds the
# committed lock. That is the same argument CLAUDE.md records for the codegen-drift step.
#
# EXIT CODES. 0 pass; 1 the lock does not satisfy the manifests; 2 infrastructure — the gate
# asserted nothing. `cargo metadata` exits 101 for a broken lock, a malformed manifest and a
# registry outage alike, so a shared code would let a crates.io outage red a REQUIRED check.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RS_DIR="$REPO_ROOT/rs"

# cargo has no distinct exit code for "the registry is down" vs "your lock is broken", so
# classify on stderr. Mirrors ci/publish-metadata/run.sh:589-604. Returns 1 for a real
# assertion failure, 2 for infrastructure.
#
# The --locked message wins FIRST and unconditionally. It is cargo's own wording for exactly
# the condition this gate exists to detect, and a truncated-lock run ALSO prints "Updating
# crates.io index" beforehand — so a network pattern evaluated first would misfile every real
# detection as infrastructure and the gate would never report red.
classify_cargo_failure() { # $1 captured-output file -> rc 1 assertion, rc 2 infrastructure
  if grep -qF 'because --locked was passed to prevent this' "$1"; then
    return 1
  fi
  if grep -qiE 'spurious network error|could not connect|connection timed out|network failure|rate limit|HTTP status 50[234]|failed to fetch|error sending request|failed to get response' "$1"; then
    return 2
  fi
  return 1
}

# Runs the assertion against the workspace at $1. Echoes cargo's captured output on failure.
# Returns 0, 1 or 2. Explicit `|| ...` rather than relying on errexit: errexit is suspended for
# the left side of an AND-OR list, so a nested failure would otherwise be swallowed.
assert_lock_satisfies_manifests() { # $1 workspace dir
  local dir="$1" out rc=0
  out="$(mktemp)" || return 2
  if ( cd "$dir" && cargo metadata --locked --format-version 1 >/dev/null ) 2>"$out"; then
    rm -f "$out"
    return 0
  fi
  classify_cargo_failure "$out" || rc=$?
  cat "$out" >&2
  rm -f "$out"
  return "$rc"
}

report() { # $1 rc
  case "$1" in
    0) echo "cargo-lock-integrity: rs/Cargo.lock satisfies every workspace manifest" ;;
    1) echo "::error::rs/Cargo.lock does not satisfy every workspace manifest. Two causes give this same result. The common one is a dependency PR that shipped a TRUNCATED lock (see SMA-601): repair it against the merge-base rather than force-pushing, and compare package counts with: grep -c '^\\[\\[package\\]\\]' rs/Cargo.lock. The other is an rs/Cargo.toml edit committed without the regenerated lock: run 'cargo metadata' in rs/ and commit the updated rs/Cargo.lock." >&2 ;;
    2) echo "::error::cargo-lock-integrity ABORTED: infrastructure error (rc=2). The gate asserted NOTHING — this is not a green result." >&2 ;;
  esac
}

# --self-test: drive classify_cargo_failure over a fixture table. Counted, never a bare pass.
self_test() {
  local failures=0 cases=0 tmp rc
  # `|| return 2` rather than letting errexit carry mktemp's status: a scratch file this mode
  # cannot create is an infrastructure failure, and rc 2 is what says "asserted nothing".
  # Matches assert_lock_satisfies_manifests above.
  tmp="$(mktemp)" || return 2

  expect_class() { # $1 name  $2 expected-rc  $3 stderr-text
    cases=$((cases + 1))
    printf '%s\n' "$3" > "$tmp"
    rc=0
    classify_cargo_failure "$tmp" || rc=$?
    if [ "$rc" -ne "$2" ]; then
      echo "self-test '$1': classify_cargo_failure returned $rc, expected $2" >&2
      failures=$((failures + 1))
    fi
  }

  expect_class 'truncated lock is an assertion failure' 1 \
    'error: cannot update the lock file /src/rs/Cargo.lock because --locked was passed to prevent this'
  # The real red path prints BOTH lines. Proves the --locked test wins over the network test.
  expect_class 'index fetch preceding a lock error is still an assertion failure' 1 \
    '    Updating crates.io index
error: cannot update the lock file /src/rs/Cargo.lock because --locked was passed to prevent this'
  expect_class 'a registry outage is infrastructure' 2 \
    'error: failed to get response from https://index.crates.io/config.json
Caused by: spurious network error (3 tries remaining)'
  expect_class 'a rate limit is infrastructure' 2 \
    'error: failed to fetch https://github.com/rust-lang/crates.io-index: rate limit exceeded'
  expect_class 'a 503 is infrastructure' 2 \
    'error: download of config.json failed: HTTP status 503'
  expect_class 'an unrecognised cargo error is an assertion failure, never a silent skip' 1 \
    'error: failed to parse manifest at /src/rs/crates/libs/paigasus-kernel/Cargo.toml'

  rm -f "$tmp"

  if [ "$cases" -ne 6 ]; then
    echo "self-test: ran $cases cases, expected 6 — a fixture row was deleted" >&2
    failures=$((failures + 1))
  fi
  if [ "$failures" -ne 0 ]; then
    echo "cargo-lock-integrity --self-test: $failures failure(s)" >&2
    return 1
  fi
  echo "cargo-lock-integrity --self-test: $cases case(s), all correct"
}

# --negative-control: prove the gate reports RED, through the SAME function the real run calls.
# A control that skips that call is the SMA-530 "control that actively lies" shape.
negative_control() {
  local tmp rc=0
  tmp="$(mktemp -d)" || return 2
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN
  git -C "$REPO_ROOT" archive HEAD rs | tar -x -C "$tmp" || return 2
  # Delete the FIRST [[package]] block. Name-free on purpose: a hard-coded crate name rots the
  # day that dependency is dropped, and any missing package is enough to make the lock
  # inconsistent with the manifests.
  python3 - "$tmp/rs/Cargo.lock" <<'PY' || return 2
import re, sys
p = sys.argv[1]
text = open(p).read()
blocks = text.split("\n[[package]]\n")
if len(blocks) < 3:
    sys.exit("negative control could not find two [[package]] blocks to mutate")
del blocks[1]
open(p, "w").write("\n[[package]]\n".join(blocks))
PY
  assert_lock_satisfies_manifests "$tmp/rs" || rc=$?
  case "$rc" in
    1) echo "cargo-lock-integrity --negative-control: reported red (rc=1) as expected" ;;
    0) echo "::error::negative control PASSED on a mutated lock — the gate cannot report red." >&2
       return 1 ;;
    *) echo "::error::negative control returned rc=$rc, not the expected 1. The control asserted NOTHING." >&2
       return 2 ;;
  esac
}

main() {
  local rc=0
  case "${1:-}" in
    --self-test)        self_test; return $? ;;
    --negative-control) negative_control; return $? ;;
    '') ;;
    *) echo "usage: run.sh [--self-test|--negative-control]" >&2; return 2 ;;
  esac
  assert_lock_satisfies_manifests "$RS_DIR" || rc=$?
  report "$rc"
  return "$rc"
}

main "$@"
