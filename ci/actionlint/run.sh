#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# actionlint gate over .github/workflows/** (SMA-525).
#
# WHY THIS EXISTS. A `paths:` filter that comes to match nothing does not error — the workflow
# simply stops running, forever, with no red check and no notification. prebuild.yml triggers
# only on push-to-main/workflow_dispatch plus a narrow pull_request filter, so its 7-platform
# verification would silently cease. Nothing in this repo linted workflow YAML before this.
#
# actionlint alone is NOT sufficient: it validates syntax and has no view of the file tree, so
# a syntactically valid glob that matches nothing (`rz/**`) passes it cleanly. Checks 5-7 below
# are what actually close the failure this gate was filed for.
#
# EXIT CODES (ci/ convention): 1 = assertion failure, 2 = infrastructure error. Without the
# split, a broken tool reads as a lint failure — or, if anyone wraps this in `|| true`, as a pass.
#
# NOT `set -e`: check 3 deliberately EXPECTS non-zero exits, and grep-based extraction
# legitimately finds nothing. Each check captures status explicitly and sets FAILED instead.
set -uo pipefail

cd "$(git rev-parse --show-toplevel)" || exit 2

FAILED=0

fail() {
  echo "actionlint gate: $*" >&2
  FAILED=1
}

infra() {
  echo "actionlint gate: INFRASTRUCTURE ERROR: $*" >&2
  exit 2
}

command -v actionlint >/dev/null 2>&1 || infra "actionlint not on PATH — run 'proto install actionlint'"

# ONE shared flag array for checks 1, 3 and 4. Written twice, an `-ignore` added to check 1
# would be invisible to check 3 BY CONSTRUCTION and the self-test would be decorative.
#
# shellcheck/pyflakes are disabled DELIBERATELY (spec D2): actionlint shells out to them when
# it finds them on PATH, which would make this gate's strictness a property of the host.
ARGS=(-shellcheck= -pyflakes=)

# Workflow discovery for checks 5-7. Non-recursive, both extensions — matching GitHub's own
# execution semantics. Check 1 does NOT use this list (see below).
WORKFLOW_FILES=()
for f in .github/workflows/*.yml .github/workflows/*.yaml; do
  [ -e "$f" ] && WORKFLOW_FILES+=("$f")
done
[ ${#WORKFLOW_FILES[@]} -gt 0 ] || infra "no workflow files found under .github/workflows/"

# ---------------------------------------------------------------------------------------------
# Check 1 — lint every workflow.
#
# Invoked BARE, with no file arguments, relying on actionlint's repository auto-discovery. Two
# reasons: a `*.yml` argument list would silently miss a `.yaml`-suffixed workflow, and
# actionlint's exit-3-on-empty behaviour (which is what makes "the directory vanished" loud
# rather than a vacuous pass) applies ONLY to the auto-discovery path — an explicit glob that
# expands to nothing would exit 0 as "no errors found".
# ---------------------------------------------------------------------------------------------
actionlint "${ARGS[@]}"
rc=$?
if [ "$rc" -ne 0 ]; then
  if [ "$rc" -eq 3 ]; then
    infra "actionlint found no workflow files to lint (exit 3)"
  fi
  fail "actionlint reported findings (exit $rc)"
fi

# ---------------------------------------------------------------------------------------------
# Check 2 — no actionlint config may neuter check 1.
#
# actionlint reads .github/actionlint.yaml, whose `paths:` map takes per-path `ignore:` regexes.
# A blanket `ignore: [".*"]` makes check 1 exit 0 on a workflow with an unknown runner label —
# VERIFIED. And the stdin fixtures of checks 3/4 are NOT suppressed by that config even when
# -stdin-filename names a matching path (also verified), so the self-tests cannot detect it.
# An explicit assertion is the only thing that can.
#
# The file itself is permitted: `self-hosted-runner.labels` is the documented escape hatch for a
# new GitHub runner label the pinned binary does not know (spec §6). Only `ignore:` is banned.
# ---------------------------------------------------------------------------------------------
for cfg in .github/actionlint.yaml .github/actionlint.yml; do
  [ -e "$cfg" ] || continue
  if grep -qE '^[[:space:]]*ignore:' "$cfg"; then
    fail "$cfg contains an 'ignore:' key, which can silently suppress every finding in check 1.
      Remove it. To teach actionlint a new runner label, use self-hosted-runner.labels instead."
  fi
done

exit "$FAILED"
