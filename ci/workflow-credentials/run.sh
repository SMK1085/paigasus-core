#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:workflow-credentials — assert no pull-request-triggered workflow DECLARES a repository
# credential. Same-repo pull requests receive repository secrets, so a credential in such a
# workflow is readable by any code the pull request introduces (SMA-407 §7 M2). "Declares" is
# the true claim and the narrower one: README.md's Non-goals section lists the paths by which a
# credential could still reach such a workflow without this gate seeing it.
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed.
#
# The checker exits 3, not 1, for an assertion failure. `uv` exits 1 on its own failures —
# MEASURED on a failed resolution both online and with UV_OFFLINE=1 — so without a distinct
# code a PyPI outage would report "a workflow declares a credential". This wrapper owns the
# translation and nothing else may.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/workflow-credentials"

die_infra() { printf 'workflow-credentials: %s\n' "$*" >&2; exit 2; }

# Preflight. `uv` absent yields 127 from the shell, which is neither 0/1/2 nor actionable.
command -v uv >/dev/null 2>&1 \
  || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"

# $@ is forwarded to the checker. Returns 0, returns 1 for a real assertion failure, and
# EXITS 2 for anything else.
run_checker() {
  local rc=0
  uv run --project "$HERE" --python '>=3.12' python3 \
    "$HERE/workflow_credentials.py" "$@" || rc=$?
  case "$rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;
  esac
}

# The wiring rows — only what needs the real tree. The rule table lives in the checker's
# --self-test, in-process, because ~31 rows through `uv run` would be ~31 subprocesses.
#
# These five discrete lines are pinned by ci_targets.py. Pinning the moon.yml INVOCATION
# alone is not enough: the repo measured two bypasses of exactly that shape on
# ci/release-parity/run.sh — neutering the flag parse so --negative-control falls through to
# the real suite, and gutting the assertion body so the control prints "reported red as
# expected" while calling nothing.
negative_control() {
  local failures=0 tmp rc
  tmp="$(mktemp -d)"

  _expect() { # $1 expected rc, $2 label, then the command
    local want="$1" label="$2"; shift 2
    local got=0
    "$@" >/dev/null 2>&1 || got=$?
    if [ "$got" != "$want" ]; then
      printf '  FAIL %s: expected rc %s, got %s\n' "$label" "$want" "$got" >&2
      failures=$((failures + 1))
    fi
  }

  # A tree with no `.github/` at all is infrastructure, not a pass: the scan root moved.
  # The mkdir deliberately creates NOTHING under it. Since SMA-593 F9 the discriminator is
  # `.github/`, so `mkdir -p .../.github` — or `.../.github/workflows` — would make this an
  # AssertionFailureError (rc 3), not the InfraError (rc 2) this row's label and _expect require.
  mkdir -p "$tmp/empty"
  _expect 2 "a tree with no .github/ is INFRA, not a vacuous pass" \
    uv run --project "$HERE" --python '>=3.12' python3 \
      "$HERE/workflow_credentials.py" "$tmp/empty"

  # A tree whose workflows exist but disagree with EXPECTED_PR_SUBJECTS is the repo's fault.
  mkdir -p "$tmp/one/.github/workflows"
  printf 'on:\n  pull_request:\njobs:\n  a:\n    runs-on: x\n' \
    >"$tmp/one/.github/workflows/ci.yml"
  _expect 3 "a shrunken subject set reds against the strict pin" \
    uv run --project "$HERE" --python '>=3.12' python3 \
      "$HERE/workflow_credentials.py" "$tmp/one"

  # THE key row. release.yml fails the credential rules and passes the gate ONLY because
  # discovery excludes it — it has no pull_request trigger. Asserting both halves is what
  # proves the trigger filter does real work rather than decorating.
  # If this reds: re-baseline. It means release.yml no longer reads a secret.
  rc=0
  grep -qE '\$\{\{[[:space:]]*secrets\.' "$REPO_ROOT/.github/workflows/release.yml" || rc=1
  if [ "$rc" != 0 ]; then
    printf '  FAIL release.yml no longer reads a secret — re-baseline this control row\n' >&2
    failures=$((failures + 1))
  fi
  # Greps the `subjects:` line the checker prints (pre-flight ruling 2). A count-only line
  # would make this row match nothing and assert nothing regardless of what discovery did.
  if bash "$0" 2>/dev/null | grep '^workflow-credentials: subjects:' | grep -q 'release.yml'; then
    printf '  FAIL release.yml appeared in the subject set; it has no pull_request trigger\n' >&2
    failures=$((failures + 1))
  fi
  if ! bash "$0" 2>/dev/null | grep -q '^workflow-credentials: subjects:'; then
    printf '  FAIL the checker printed no subjects line — the row above cannot assert\n' >&2
    failures=$((failures + 1))
  fi

  # The 3 -> 1 translation itself. A checker rc 3 must reach the caller as 1, not 3.
  _expect 1 "the wrapper maps a checker assertion (3) onto the repo contract (1)" \
    run_checker "$tmp/one"

  rm -rf "$tmp"
  if [ "$failures" -gt 0 ]; then
    printf 'workflow-credentials negative control: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== workflow-credentials negative control passed ==\n'
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        MODE=selftest; shift ;;
    --negative-control) MODE=negctl;   shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_checker --self-test ;;
  check)    run_checker "$REPO_ROOT" ;;
  negctl)   negative_control ;;
esac
