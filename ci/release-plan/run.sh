#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:release-plan — decide whether a push to `main` has anything to release, so the release
# workflow's `plan` job can skip its ~15-minute build matrix when it does not. The decision is
# TAG EXISTENCE, not a `release-plz release --dry-run` read: see release_plan.py's module
# docstring for why the dry-run reading is silently, permanently wrong (measurement M6).
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed — EXCEPT --github-output,
# which always exits 0. See the comment on that arm.
#
# The checker exits 3, not 1, for an assertion failure. `uv` exits 1 on its own failures, so
# without a distinct code a PyPI outage would read as "the repo is wrong". This wrapper owns
# the 3 -> 1 translation and nothing else may.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/release-plan"

die_infra() { printf 'release-plan: %s\n' "$*" >&2; exit 2; }

# Preflight. `uv` absent yields 127 from the shell, which is neither 0/1/2 nor actionable.
command -v uv >/dev/null 2>&1 \
  || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"

# $@ is forwarded to the checker. Returns 0, returns 1 for a real assertion failure, and
# EXITS 2 for anything else.
run_checker() {
  local rc=0
  uv run --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" "$@" || rc=$?
  case "$rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;
  esac
}

# THE RUNTIME ARM, and the one place in this repo where a checker failure must NOT fail its
# caller. A failed `plan` job SKIPS its dependents rather than building them — GitHub applies an
# implicit success() to a job-level `if:` with no status function — so a broken decision that
# exited non-zero would stop the release entirely. Fail-safe here means: write false, warn
# loudly, exit 0, and let the matrix build. The --self-test/--negative-control/--assert modes
# keep the normal contract, and CI runs those.
github_output() {
  local rc=0 out
  out="$(uv run --project "$HERE" --python '>=3.12' python3 \
    "$HERE/release_plan.py" --event-name "${GITHUB_EVENT_NAME:-}" "$REPO_ROOT" 2>&1)" || rc=$?
  printf '%s\n' "$out"
  if [ "$rc" -ne 0 ] || ! printf '%s\n' "$out" | grep -qE '^nothing_to_release=(true|false)$'; then
    printf '::warning::release-plan could not decide (rc=%s) — building, which is the fail-safe direction\n' "$rc"
    printf 'nothing_to_release=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"
    exit 0
  fi
  printf '%s\n' "$out" | grep -E '^nothing_to_release=(true|false)$' >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
}

# The wiring rows — only what needs the real tree. The rule table lives in the checker's
# --self-test, in-process, because it needs no filesystem beyond the two temp trees it builds
# itself. This control has one extra job the sibling gates do not: proving the fail-safe
# --github-output arm actually flips direction with the event name, not merely that it exits 0
# for both — a checker wired to a constant `nothing_to_release=false` would pass every OTHER
# row here.
negative_control() {
  local failures=0 tmp out

  _expect() { # $1 expected rc, $2 label, then the command
    local want="$1" label="$2"; shift 2
    local got=0
    "$@" >/dev/null 2>&1 || got=$?
    if [ "$got" != "$want" ]; then
      printf '  FAIL %s: expected rc %s, got %s\n' "$label" "$want" "$got" >&2
      failures=$((failures + 1))
    fi
  }

  tmp="$(mktemp -d)"

  # Row 1 — the 3 -> 1 translation itself. A tree with no `rs/` at all cannot resolve any crate
  # manifest, so releasable_packages() raises Inconclusive, --assert exits 3, and run_checker
  # must map that onto the repo contract's 1, not pass the 3 through and not silently mask it
  # as an infra failure.
  mkdir -p "$tmp/empty"
  _expect 1 "the wrapper maps a checker assertion (3) onto the repo contract (1)" \
    run_checker --assert "$tmp/empty"

  # Row 2 — the self-test itself must still be capable of catching a broken fixture table. This
  # is a smoke check that --self-test wiring reaches the real FIXTURES list, not a fixture retest
  # (the table already re-runs on every CI invocation of --self-test).
  _expect 0 "the self-test passes against the real fixture table" \
    run_checker --self-test

  # Row 3 — the state that would otherwise skip: every tag is already cut, but the event is a
  # workflow_dispatch. If --github-output ever ignored the event name, this is where it would
  # show: a dispatch must still print nothing_to_release=false.
  out="$(GITHUB_EVENT_NAME=workflow_dispatch bash "$0" --github-output 2>&1)" || true
  if ! printf '%s\n' "$out" | grep -q '^nothing_to_release=false$'; then
    printf '  FAIL a workflow_dispatch run did not print nothing_to_release=false\n' >&2
    printf '  --- output ---\n%s\n' "$out" >&2
    failures=$((failures + 1))
  fi

  # Row 4 — the real repo, on a push, today: every tag is cut, so this must read true. Without
  # both this row and row 3, the control cannot tell a working decision from one wired to a
  # constant in either direction.
  out="$(GITHUB_EVENT_NAME=push bash "$0" --github-output 2>&1)" || true
  if ! printf '%s\n' "$out" | grep -q '^nothing_to_release=true$'; then
    printf '  FAIL a push run against the real, fully-tagged repo did not print nothing_to_release=true\n' >&2
    printf '  --- output ---\n%s\n' "$out" >&2
    failures=$((failures + 1))
  fi

  rm -rf "$tmp"
  if [ "$failures" -gt 0 ]; then
    printf 'release-plan negative control: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== release-plan negative control passed ==\n'
}

MODE=
while [ $# -gt 0 ]; do
  case "$1" in
    --github-output)     MODE=output; shift ;;
    --self-test)         MODE=selftest; shift ;;
    --negative-control)  MODE=negctl; shift ;;
    --assert)            MODE=assert; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  output)   github_output ;;
  selftest) run_checker --self-test ;;
  negctl)   negative_control ;;
  assert)   run_checker --assert "$REPO_ROOT" ;;
  *)        die_infra "one mode is required: --github-output | --self-test | --negative-control | --assert" ;;
esac
